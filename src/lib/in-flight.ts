/**
 * 「同一时刻最多一个在途请求」的守卫（纯逻辑，无 React / DOM 依赖）。
 *
 * 为什么单独抽出来：页面上的「刷新中…」+ `disabled` 只挡得住**按规矩点**的人，挡不住
 * 合成事件、键盘重复触发或动作链（复制 / 同步 / 清理完成后的 onDone）的并发调用。
 * 把「是否允许发起」收进数据层，才能做到「绕过 UI 也只发一次」。
 *
 * 两种语义：
 *
 * - 默认（`force: false`）：已有在途请求时直接返回（复用那一次），不重发。
 * - `force: true`：已有在途请求时**不并发、也不复用旧结果**，排队等它结束后再执行一次
 *   —— 动作链必须看到动作之后的数据。同一轮里多次 `force` 合并成一次重跑，排队上限恒为 1，
 *   因此不会出现请求风暴。
 *
 * 与页面里的 `requestId` 分工：这里管「要不要发」，`requestId` 管「回来的包还能不能写」。
 */

export interface RunOptions {
  /** `true` = 在途时排队重跑一次；`false`（默认）= 复用当前在途请求，不重发。 */
  force?: boolean;
}

export interface InFlightGuard {
  /** 当前是否已有一次执行在途（含排队重跑期间）。 */
  isRunning(): boolean;
  /** 抢占执行权：空闲则置为在途并返回 `true`，否则返回 `false`。 */
  tryStart(): boolean;
  /**
   * 结束一次执行。
   *
   * 返回 `true` = 有排队中的重跑：守卫**继续被占用**（避免两次执行之间被第三个调用插队），
   * 调用方必须再执行一次；返回 `false` = 已释放，等待中的 `force` 调用者全部完成。
   */
  finish(): boolean;
  /** 登记「当前这次结束后再跑一次」；没有在途执行时直接忽略（排队没有意义）。 */
  defer(): void;
  /**
   * 执行一次任务：空闲则立刻执行；在途时按 `force` 决定是复用还是排队重跑。
   *
   * 任务抛错时守卫一律复位并释放排队的调用者（本项目的调用方自己捕获错误，不靠这里兜）。
   */
  run(task: () => Promise<void>, options?: RunOptions): Promise<void>;
}

export function createInFlightGuard(): InFlightGuard {
  let running = false;
  let deferred = false;
  let waiters: Array<() => void> = [];

  function release(): void {
    running = false;
    deferred = false;
    const pending = waiters;
    waiters = [];
    for (const resolve of pending) resolve();
  }

  function tryStart(): boolean {
    if (running) return false;
    running = true;
    return true;
  }

  function finish(): boolean {
    // 没在途时被调用：没有可结束的执行，也不该凭空产生一次重跑。
    if (!running) return false;
    if (!deferred) {
      release();
      return false;
    }
    deferred = false;
    return true;
  }

  function defer(): void {
    if (!running) return;
    deferred = true;
  }

  async function run(task: () => Promise<void>, options: RunOptions = {}): Promise<void> {
    const force = options.force === true;
    if (!tryStart()) {
      if (!force) return;
      defer();
      // 每个强制调用各自等这一轮（合并后的）重跑结束后再返回。
      await new Promise<void>((resolve) => {
        waiters.push(resolve);
      });
      return;
    }
    try {
      for (;;) {
        await task();
        if (!finish()) return;
      }
    } catch (error) {
      release();
      throw error;
    }
  }

  return {
    isRunning: () => running,
    tryStart,
    finish,
    defer,
    run,
  };
}
