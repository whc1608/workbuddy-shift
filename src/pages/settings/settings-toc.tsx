import { useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import type { SettingsGrouped } from "./sections";

interface Props {
  /** 已按搜索过滤、且已按当前环境过滤的分组节。 */
  groups: SettingsGrouped[];
  /** 宽屏下的固定宽度栏：由页面用 grid 定义列，这里只负责 sticky 与内容。 */
  className?: string;
}

/**
 * 判定线：视口顶部往下多少像素算「这一节的标题已经越过」。
 *
 * 与节标题上的 `scroll-mt-24`（96px，见 `rows.tsx` 的 `SettingsGroup`）对齐并留一点余量：
 * 跳转落地后标题停在 96px，`96 <= 120` 才判定为「就是这一节」。若判定线比 96 还小，
 * 刚滚过去的这一节会被判成「还没到」，active 会掉回上一节。
 */
export const TOC_ACTIVE_OFFSET = 120;

/** 一节标题在视口里的位置（只需要 `id` 与 `top`；`top` 由 `getBoundingClientRect()` 给出）。 */
export interface TocSectionBox {
  id: string;
  top: number;
}

/**
 * 按**节标题相对视口顶部的位置**选当前节（纯函数，便于单测）。
 *
 * 规则，按优先级：
 * 1. 已经滚到底（`atBottom`）→ 最后一节。最后一节的卡片通常不够高，标题可能永远
 *    越不过判定线，若不做这一条，「滚到底」时高亮会停在倒数第二节；
 * 2. 取**最后一个** `top <= viewportTop + offset` 的节——即使它是张很短的卡，
 *    只要标题已经越线就归它，不会被紧接着的下一节抢走（旧实现按「最大交叉比」选，
 *    短卡在视口里占比小，正中这个坑）；
 * 3. 一个都没越线（还在顶部）→ 第一节。
 */
export function pickActiveSection(
  sections: readonly TocSectionBox[],
  viewportTop: number,
  viewportHeight: number,
  offset: number = TOC_ACTIVE_OFFSET,
  atBottom = false,
): string {
  if (sections.length === 0) return "";
  if (atBottom) return sections[sections.length - 1].id;

  // 判定线不能越过视口下沿（窗口被压得很矮时 offset 会失真）。
  const line = viewportTop + Math.min(offset, viewportHeight);
  let active = sections[0].id;
  for (const section of sections) {
    if (section.top > line) break;
    active = section.id;
  }
  return active;
}

/**
 * 设置页目录（宽屏专用，`xl` 以上显示）。
 *
 * 三件事：
 * 1. **跳转**：点条目滚动到对应节。用 `scrollIntoView` 而不是 `<a href="#id">`，
 *    避免默认跳转把锚点顶到视口最上方（被 sticky 顶栏盖住）；URL 用 `replaceState`
 *    同步，不产生历史记录，也不会触发 HashRouter 的 hashchange（演示站点用的是 HashRouter，
 *    那里的 hash 是路由本身：`#/settings`，改成 `#settings-startup` 会把页面甩回首页）。
 * 2. **高亮跟随**：只有一个事实来源——滚动时用 rAF 节流重算一次「标题越线的最后一节」
 *    （`pickActiveSection`）。不用 IntersectionObserver：观察者按「回调批次里可见的条目」
 *    回写 active，平滑滚动结束时最后一批往往只有下一节，于是高亮漂到下一节（T05 缺陷 A1）。
 *    位置判定与「交叉比大小」无关，短卡不会被抢。
 * 3. **深链**：挂载时读一次 hash（`/settings#settings-startup`），命中已知节就滚过去，
 *    并把 active 钉在该节上，直到用户真的用手滚（wheel / touchmove / 滚动翻页键）——
 *    卡片里的数据加载完成后行高会变，标题可能从 96px 漂到 130px 之外，若立刻重算，
 *    高亮会掉到上一节。用户一动手就解除钉住，回到正常跟随。
 */
export function SettingsToc({ groups, className }: Props) {
  const [activeId, setActiveId] = useState<string>(groups[0]?.sections[0]?.id ?? "");
  /** 深链钉住的节 id；非 null 时重算不回写 active（见上方注释 3）。 */
  const pinnedRef = useRef<string | null>(null);

  // 目录项的 id 串：用它当 effect 依赖。groups 每次渲染都是新数组，
  // 直接当依赖会让监听器每帧重建（滚动时反复 add/remove 事件）。
  const idsKey = groups.flatMap((group) => group.sections.map((section) => section.id)).join(",");

  useEffect(() => {
    if (typeof window === "undefined") return;
    const ids = idsKey ? idsKey.split(",") : [];
    const targets = ids
      .map((id) => document.getElementById(id))
      .filter((element): element is HTMLElement => element !== null);
    if (targets.length === 0) return;

    // 滚动发生在 `<main class="overflow-y-auto">` 里，不是文档本身：
    // 判定要拿滚动容器的视口顶（`rect.top + clientTop`）与 `atBottom`。
    const scroller = findScrollContainer(targets[0]);
    const isDocument = scroller === document.scrollingElement || scroller === document.documentElement;

    const recompute = () => {
      if (pinnedRef.current) return;
      const viewportTop = isDocument ? 0 : scroller.getBoundingClientRect().top + scroller.clientTop;
      const viewportHeight = isDocument ? window.innerHeight : scroller.clientHeight;
      // 内容比容器还短时 `scrollHeight === clientHeight`，那不是「滚到底」而是「没得滚」。
      const atBottom =
        scroller.scrollHeight > scroller.clientHeight + 2 &&
        scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      const boxes: TocSectionBox[] = targets.map((element) => ({
        id: element.id,
        top: element.getBoundingClientRect().top,
      }));
      const next = pickActiveSection(boxes, viewportTop, viewportHeight, TOC_ACTIVE_OFFSET, atBottom);
      if (next) setActiveId(next);
    };

    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        recompute();
      });
    };

    // 折叠面板展开/收起会改行高但不产生 scroll 事件，用尺寸变化兜底重算。
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => schedule());
    const contentBox = targets[0].parentElement?.parentElement;
    if (observer && contentBox) observer.observe(contentBox);

    scroller.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    schedule();

    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      observer?.disconnect();
      scroller.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
    };
  }, [idsKey]);

  // 深链只在挂载时读一次：之后用户点目录不再改地址语义，避免与滚动高亮互相打架。
  useEffect(() => {
    if (typeof window === "undefined") return;
    const hash = window.location.hash.replace(/^#/, "");
    if (!hash || hash.startsWith("/")) return;
    const frame = window.requestAnimationFrame(() => {
      const element = document.getElementById(hash);
      if (!element) return;
      pinnedRef.current = hash;
      setActiveId(hash);
      element.scrollIntoView({ block: "start" });
    });

    // 用户真的动手滚了才解除钉住：程序滚动（上面的 scrollIntoView）与数据加载后的
    // 行高变化都不该解除，否则深链的 active 会被重算覆盖成上一节。
    const listeners: Array<[string, EventListener]> = [];
    const unpin = () => {
      pinnedRef.current = null;
      for (const [type, listener] of listeners) window.removeEventListener(type, listener);
    };
    const onKeyDown: EventListener = (event) => {
      if (SCROLL_KEYS.has((event as KeyboardEvent).key)) unpin();
    };
    listeners.push(["wheel", unpin], ["touchmove", unpin], ["keydown", onKeyDown]);
    for (const [type, listener] of listeners) {
      window.addEventListener(type, listener, { passive: true });
    }

    return () => {
      window.cancelAnimationFrame(frame);
      for (const [type, listener] of listeners) window.removeEventListener(type, listener);
    };
  }, []);

  function jump(id: string) {
    const element = document.getElementById(id);
    if (!element) return;
    pinnedRef.current = null;
    setActiveId(id);
    element.scrollIntoView({ block: "start", behavior: "smooth" });
    if (typeof window !== "undefined" && !window.location.hash.startsWith("#/")) {
      window.history.replaceState(null, "", `#${id}`);
    }
    // 不再用定时器「交还」：平滑滚动的每一帧都会重算，落点标题停在 96px <= 判定线 120px，
    // 重算结果必然还是这一节；即便用户中途插进来滚走，位置判定也会如实跟到新的节。
  }

  // `top-10`（40px）而不是贴顶：macOS 桌面壳在窗口顶部固定了一条 32px 高的
  // `data-tauri-drag-region`（`src/App.tsx` 的 hasUnifiedTitleBar），贴顶的目录
  // 最上面几行会落进那条拖拽带里点不动。
  return (
    <nav aria-label="设置目录" className={cn("sticky top-10 self-start", className)}>
      <div className="space-y-5">
        {groups.map((group) => (
          <div key={group.group.id}>
            <div className="px-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground/70">
              {group.group.title}
            </div>
            <ul className="mt-1.5 space-y-0.5">
              {group.sections.map((section) => {
                const active = section.id === activeId;
                return (
                  <li key={section.id}>
                    <button
                      type="button"
                      onClick={() => jump(section.id)}
                      aria-current={active ? "true" : undefined}
                      className={cn(
                        "w-full cursor-pointer truncate rounded-md px-2 py-1.5 text-left text-[13px] leading-5 outline-none transition-colors",
                        "focus-visible:ring-2 focus-visible:ring-ring/50",
                        active
                          ? "bg-foreground/[0.06] font-medium text-foreground"
                          : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                      )}
                    >
                      {section.title}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </nav>
  );
}

/** 触发「解除深链钉住」的按键：滚动类按键，普通输入不算。 */
const SCROLL_KEYS = new Set([
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
 * 找最近的可滚动祖先；都没有就退回文档。
 *
 * 设置页的 `main` 是 `overflow-y-auto`，判定要用它的视口顶与 scrollTop/scrollHeight，
 * 用 `window` 的滚动量会永远是 0（文档不滚，见 T05 报告第 1 节）。
 */
function findScrollContainer(element: HTMLElement): HTMLElement {
  let node = element.parentElement;
  while (node && node !== document.documentElement) {
    const overflowY = window.getComputedStyle(node).overflowY;
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay") return node;
    node = node.parentElement;
  }
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}
