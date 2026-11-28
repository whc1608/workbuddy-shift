/**
 * 目录跳转的**落点校正**（T05 K1），从 `settings-toc.tsx` 拆出以守住 500 行结构阈值。
 *
 * 背景：`scrollIntoView({ behavior: "smooth" })` 只在调用瞬间算一次目标偏移；跳转期间
 * 上方卡片仍在加载/状态切换，长高 89px 后标题从 96px 漂到 185px，越过判定线后高亮就落到
 * 上一节。钉住只能保住「平滑期间别回写 active」，落点本身要等停稳后再量一次并校正。
 *
 * 这里只放纯函数与 DOM 时序，不依赖 React；`settings-toc.tsx` 负责把滚动容器、目标元素
 * 与钉住状态传进来。
 */

/**
 * 落点目标：节标题距滚动容器可视顶多少像素算「跳到了」。
 *
 * 与节标题上的 `scroll-mt-24`（96px，见 `rows.tsx` 的 `SettingsGroup`）一致——
 * `scrollIntoView({ block: "start" })` 就是把标题停在这里。`TOC_ACTIVE_OFFSET`（120px，
 * 见 `settings-toc.tsx`）是判定容差线，**不是**落点线：按 120px 校正会把每次正常跳转
 * 整体往下推 24px。
 */
export const TOC_LANDING_TOP = 96;

/** 落点校正的容差（px）：偏差在 ±2px 内视为命中，避免平滑收尾的像素抖动来回拉锯。 */
export const LANDING_TOLERANCE = 2;

/** 平滑滚动是否停下的判断：最后一次 `scroll` 之后这么久没有新滚动就算停了（`scrollend` 兜底）。 */
const SCROLL_SETTLE_MS = 250;
/** 总兜底：不管有没有滚动事件，最多等这么久就量落点。 */
const LANDING_TIMEOUT_MS = 1200;
/** 点击后一直没有 `scroll`（本来就停在目标上）时的提前检查。 */
const LANDING_FIRST_CHECK_MS = 400;
/** 校正后观察布局再变的窗口：窗口内尺寸变化又把标题推走，允许再校正一次。 */
const LANDING_RELAYOUT_MS = 300;
/** 落点校正次数上限：一次校正 + 一次布局补偿。 */
const MAX_LANDING_CORRECTIONS = 2;
/**
 * 落点确认后继续看守的时长：覆盖「数据加载完成得比滚动晚」的二次长高。
 *
 * QA 实测：加载后 0.56s 点「凭据」，1.09s 落点正好 96px；2.59s 时上方卡片长高，
 * 标题被推到 133px（差 37px），而滚动早已停稳、300ms 的校正窗口也已经结束。
 * 高亮仍是对的（钉住/判定没坏），但落点偏低，属于同一根因的尾巴。
 */
const LAYOUT_WATCH_MS = 10000;
/** 看守期间最多补几次：防止持续抖动时无限拉锯。 */
const MAX_WATCH_ADJUSTMENTS = 8;
/** 看守轮询间隔：内容长高不一定触发 scroll，轮询最稳（10s 内约 66 次测量，代价可忽略）。 */
const LAYOUT_POLL_MS = 150;

/** 触发「用户要自己滚」的按键：滚动类按键，普通输入不算。深链解钉与取消落点校正共用。 */
export const SCROLL_KEYS = new Set([
  "ArrowUp",
  "ArrowDown",
  "PageUp",
  "PageDown",
  "Home",
  "End",
  " ",
  "Spacebar",
]);

/**
 * 落点校正量（纯函数，便于单测）：目标标题当前在 `targetTop`，期望停在
 * `viewportTop + offset`，返回应追加到 `scrollTop` 的像素数。
 *
 * 正数 = 还差一截，继续往下滚；负数 = 滚过头了，往回退；`±LANDING_TOLERANCE` 内返回 0。
 */
export function landingDelta(targetTop: number, viewportTop: number, offset: number): number {
  const delta = targetTop - (viewportTop + offset);
  return Math.abs(delta) <= LANDING_TOLERANCE ? 0 : delta;
}

export interface LandingCorrectionOptions {
  /** 滚动容器（设置页的 `main`）。 */
  scroller: HTMLElement;
  /** 这次跳转的目标节标题。 */
  target: HTMLElement;
  /** 这次跳转钉住的节 id；`pinnedRef.current` 不再是它就说明已被新跳转/用户滚动接管。 */
  id: string;
  pinnedRef: { current: string | null };
  setActiveId: (id: string) => void;
}

/**
 * 平滑滚动结束后的**一次性落点校正**。
 *
 * 时序：
 * - 优先等滚动容器的 `scrollend`；不支持时用「最后一次 scroll 后 250ms」兜底；
 * - 另设 1200ms 总兜底；点击后 400ms 内一次 scroll 都没有（本来就停在目标上）也直接检查；
 * - 量出的差值超过容差 → 在 rAF 里 `scrollTop += delta` **非平滑**校正一次；
 * - 校正后观察 300ms：若布局又变、把标题推离落点，允许再校正一次（总次数 ≤ 2）；
 * - 落点确认后再挂 **10s 看守**：内容在跳转之后才长高（实测 2.5s，延迟接口可到 5s）时，把标题对回落点线；
 *   看守只修落点，不碰 active，也不重新钉住；
 * - wheel / touchmove / 滚动翻页键 / 外部滚动 / 新跳转 → 立刻取消校正与看守，用户的滚动永远赢。
 *
 * 返回取消函数（新跳转、卸载时调用）：既停校正，也停 10s 看守。
 */
export function armLandingCorrection({
  scroller,
  target,
  id,
  pinnedRef,
  setActiveId,
}: LandingCorrectionOptions): () => void {
  if (typeof window === "undefined") return () => {};

  const isDocument = scroller === document.scrollingElement || scroller === document.documentElement;
  const contentBox = target.parentElement?.parentElement ?? null;
  const timers = new Set<number>();
  let observer: ResizeObserver | null = null;
  let raf = 0;
  let idleTimer = 0;
  let firstTimer = 0;
  let watchTimer = 0;
  let capTimer = 0;
  let corrections = 0;
  let finished = false;
  /** 落点看守的停止函数（落点确认后启动，覆盖 10s 内的延迟布局）。 */
  let stopWatch: (() => void) | null = null;

  const later = (run: () => void, ms: number) => {
    const handle = window.setTimeout(() => {
      timers.delete(handle);
      run();
    }, ms);
    timers.add(handle);
    return handle;
  };

  const cancelTimer = (handle: number) => {
    if (!handle) return;
    window.clearTimeout(handle);
    timers.delete(handle);
  };

  const stillMine = () => pinnedRef.current === id;

  const measure = () => {
    const viewportTop = isDocument ? 0 : scroller.getBoundingClientRect().top + scroller.clientTop;
    return landingDelta(target.getBoundingClientRect().top, viewportTop, TOC_LANDING_TOP);
  };

  /** 拆掉监听器与定时器。不动钉住状态（可能是被新跳转接管）。 */
  function stop() {
    // 看守独立于校正生命周期：即便校正早已收尾，新跳转/卸载也要能停掉它。
    stopWatch?.();
    stopWatch = null;
    if (finished) return false;
    finished = true;
    scroller.removeEventListener("scroll", onScroll);
    scroller.removeEventListener("scrollend", onSettled);
    window.removeEventListener("wheel", onUserInput);
    window.removeEventListener("touchmove", onUserInput);
    window.removeEventListener("keydown", onKeyDown);
    observer?.disconnect();
    observer = null;
    if (raf) window.cancelAnimationFrame(raf);
    raf = 0;
    for (const handle of timers) window.clearTimeout(handle);
    timers.clear();
    return true;
  }

  /**
   * 收尾并解除钉住，回到正常跟随。
   *
   * `watchLate` 为 true 时再挂一个 10s 的**落点看守**：只修落点，不碰 active、不重新钉住，
   * 用户一滚就撤。深链与用户主动打断（wheel/touch/滚动键）不走看守。
   */
  function finish(watchLate = false) {
    if (!stop()) return;
    if (stillMine()) pinnedRef.current = null;
    if (watchLate) startLayoutWatch();
  }

  /**
   * 落点确认后的短期布局看守：内容在跳转之后才长高时，把标题重新对回落点线。
   *
   * 为什么不用 ResizeObserver 观察 `contentBox`：它只是第一节的容器，后面卡片的
   * 高度变化不会改变它自己的盒子尺寸（实测 2.5s 那次长高就没触发）。这里改用轮询
   * 直接量目标标题，任何位置的长高都能看见。
   *
   * 为什么**不**拿 `scrollTop` 变化当「用户滚动」：布局长高时 Chrome 的 scroll anchoring
   * 也会改 `scrollTop`，与看自己的校正混在一起（实测一次 +52px 的校正被误判成用户滚动，
   * 看守提前退出，留下 37px 偏差）。用户输入只认真实事件：wheel / touchmove / 滚动键 /
   * 指针按下（拖滚动条、点滚动区域），以及新跳转。
   */
  function startLayoutWatch() {
    if (typeof window === "undefined") return;
    let stopped = false;
    let adjustments = 0;
    const until = Date.now() + LAYOUT_WATCH_MS;
    let timer = 0;

    const cleanup = () => {
      if (stopped) return;
      stopped = true;
      window.clearTimeout(timer);
      window.removeEventListener("wheel", cleanup);
      window.removeEventListener("touchmove", cleanup);
      window.removeEventListener("keydown", onKeyDownWatch);
      scroller.removeEventListener("pointerdown", cleanup);
      window.removeEventListener("mousedown", cleanup);
      if (stopWatch === cleanup) stopWatch = null;
    };
    const onKeyDownWatch = (event: Event) => {
      if (SCROLL_KEYS.has((event as KeyboardEvent).key)) cleanup();
    };
    const tick = () => {
      if (stopped) return;
      // 目标被搜索/条件渲染摘掉：别再量一个已经不在文档里的节点。
      if (!target.isConnected) {
        cleanup();
        return;
      }
      // 先量再判超时：截止那一刻如果正好又长高，也要把这次补掉再收工。
      const delta = measure();
      if (delta !== 0 && adjustments < MAX_WATCH_ADJUSTMENTS) {
        adjustments += 1;
        scroller.scrollTop += delta;
      }
      if (Date.now() >= until || adjustments >= MAX_WATCH_ADJUSTMENTS) {
        cleanup();
        return;
      }
      timer = window.setTimeout(tick, LAYOUT_POLL_MS);
    };

    window.addEventListener("wheel", cleanup, { passive: true });
    window.addEventListener("touchmove", cleanup, { passive: true });
    window.addEventListener("keydown", onKeyDownWatch, { passive: true });
    // 拖滚动条 / 在页面上按下鼠标：算用户接管，立刻停。
    scroller.addEventListener("pointerdown", cleanup, { passive: true });
    window.addEventListener("mousedown", cleanup, { passive: true });
    timer = window.setTimeout(tick, LAYOUT_POLL_MS);
    stopWatch = cleanup;
  }

  /** 用户真的动手滚了：待执行的校正作废，钉住也解除。 */
  function onUserInput() {
    finish();
  }

  function onKeyDown(event: Event) {
    if (SCROLL_KEYS.has((event as KeyboardEvent).key)) finish();
  }

  function onSettled() {
    correct();
  }

  function onScroll() {
    cancelTimer(firstTimer);
    firstTimer = 0;
    cancelTimer(idleTimer);
    idleTimer = later(correct, SCROLL_SETTLE_MS);
  }

  /** 落点已经量过并校正：不再需要等「滚动停稳」，也别让校正自己产生的 scroll 事件再触发一次。 */
  function detachSettleWatchers() {
    scroller.removeEventListener("scroll", onScroll);
    scroller.removeEventListener("scrollend", onSettled);
    cancelTimer(firstTimer);
    firstTimer = 0;
    cancelTimer(idleTimer);
    idleTimer = 0;
    cancelTimer(capTimer);
    capTimer = 0;
  }

  function correct() {
    if (finished) return;
    if (!stillMine()) {
      // 已被新跳转接管：拆掉自己，别碰对方钉住的那一节。
      stop();
      return;
    }
    const delta = measure();
    if (delta === 0 || corrections >= MAX_LANDING_CORRECTIONS) {
      // 落点已经对了，但数据可能还在长高：交给 10s 看守继续盯。
      finish(true);
      return;
    }
    // 目标就在页面最顶上（如「外观」）：标题本来就够不到落点线（滚不动负距离），
    // 往下补 100+px 会把页头滚出视野，那不是校正而是新的错误落点。
    if (delta > 0 && scroller.scrollTop <= 1) {
      finish(false);
      return;
    }
    corrections += 1;
    cancelTimer(watchTimer);
    watchTimer = 0;
    detachSettleWatchers();
    raf = window.requestAnimationFrame(() => {
      raf = 0;
      if (finished) return;
      if (!stillMine()) {
        stop();
        return;
      }
      // 帧内再量一次：settle 到这一帧之间若又长了一截，按最新差值一次到位。
      const apply = measure();
      if (apply !== 0) {
        scroller.scrollTop += apply;
        setActiveId(id);
      }
      watchRelayout();
    });
  }

  /** 校正只做一次；但 300ms 内布局又长高把标题推走时，允许再补一次。 */
  function watchRelayout() {
    if (corrections >= MAX_LANDING_CORRECTIONS || !contentBox || typeof ResizeObserver === "undefined") {
      finish(true);
      return;
    }
    observer = new ResizeObserver(() => {
      if (finished || !stillMine()) return;
      // 尺寸变了但没把标题推离落点：继续等窗口结束。
      if (measure() === 0) return;
      observer?.disconnect();
      observer = null;
      correct();
    });
    observer.observe(contentBox);
    // 300ms 内的尺寸补偿结束不等于「布局稳定」：转交 10s 看守继续盯延迟加载。
    watchTimer = later(() => finish(true), LANDING_RELAYOUT_MS);
  }

  scroller.addEventListener("scroll", onScroll, { passive: true });
  scroller.addEventListener("scrollend", onSettled);
  window.addEventListener("wheel", onUserInput, { passive: true });
  window.addEventListener("touchmove", onUserInput, { passive: true });
  window.addEventListener("keydown", onKeyDown, { passive: true });
  // 点击时已经停在目标位置（没有可滚的）：400ms 无 scroll 事件就直接检查收尾。
  firstTimer = later(correct, LANDING_FIRST_CHECK_MS);
  capTimer = later(correct, LANDING_TIMEOUT_MS);

  return stop;
}
