import { describe, expect, it } from "vitest";

import { createInFlightGuard } from "./in-flight";

/**
 * 「进行中短路」守卫的单测。
 *
 * 覆盖四件在线程上容易出事的行为：并发只发一次、结束可再发、`force` 排队（且合并）、
 * 抛错也要复位。页面层（`use-session-groups`）只负责把 `run` 接上 `setRefreshing`，
 * 判定逻辑全在这里，所以这里测到的就是生产逻辑本身。
 */

/** 可手动放行的闸门：让「在途」这件事在测试里可观察、可复现。 */
function createGate(): { promise: Promise<void>; open: () => void } {
  let open!: () => void;
  const promise = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { promise, open };
}

describe("createInFlightGuard", () => {
  it("并发调用三次只执行一次（默认复用，不重发）", async () => {
    const guard = createInFlightGuard();
    const gate = createGate();
    let calls = 0;
    const task = async (): Promise<void> => {
      calls += 1;
      await gate.promise;
    };

    const first = guard.run(task);
    const second = guard.run(task);
    const third = guard.run(task);
    expect(guard.isRunning()).toBe(true);

    // 后两次立刻返回：既不排队也不重发。
    await Promise.all([second, third]);
    expect(calls).toBe(1);

    gate.open();
    await first;
    expect(guard.isRunning()).toBe(false);
    expect(calls).toBe(1);
  });

  it("上一次结束后可以再执行", async () => {
    const guard = createInFlightGuard();
    let calls = 0;
    const task = async (): Promise<void> => {
      calls += 1;
    };

    await guard.run(task);
    await guard.run(task);

    expect(calls).toBe(2);
    expect(guard.isRunning()).toBe(false);
  });

  it("force 排队：等在途结束后再执行一次，多次 force 合并成一次重跑", async () => {
    const guard = createInFlightGuard();
    const gate = createGate();
    const order: string[] = [];
    let calls = 0;
    const task = async (): Promise<void> => {
      calls += 1;
      const current = calls;
      order.push(`start-${current}`);
      // 第一次卡住（模拟真实网络请求），重跑立刻返回。
      if (current === 1) await gate.promise;
      order.push(`end-${current}`);
    };

    const owner = guard.run(task);
    const forcedA = guard.run(task, { force: true });
    const forcedB = guard.run(task, { force: true });
    expect(guard.isRunning()).toBe(true);

    // 排队中的调用者不能在在途请求结束前返回。
    let settled = false;
    void forcedA.then(() => {
      settled = true;
    });
    await Promise.resolve();
    expect(settled).toBe(false);

    gate.open();
    await Promise.all([owner, forcedA, forcedB]);

    expect(order).toEqual(["start-1", "end-1", "start-2", "end-2"]);
    expect(calls).toBe(2);
    expect(guard.isRunning()).toBe(false);
  });

  it("任务抛错也会复位：守卫释放，之后还能再执行", async () => {
    const guard = createInFlightGuard();
    await expect(
      guard.run(async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");

    expect(guard.isRunning()).toBe(false);

    let calls = 0;
    await guard.run(async () => {
      calls += 1;
    });
    expect(calls).toBe(1);
  });

  it("在途任务抛错时，排队的 force 调用者也会被释放（不悬挂）", async () => {
    const guard = createInFlightGuard();
    const gate = createGate();
    const owner = guard.run(async () => {
      await gate.promise;
      throw new Error("boom");
    });
    const forced = guard.run(async () => {
      throw new Error("不应执行");
    }, { force: true });

    gate.open();

    await expect(owner).rejects.toThrow("boom");
    await expect(forced).resolves.toBeUndefined();
    expect(guard.isRunning()).toBe(false);
  });

  it("没有在途执行时 defer 被忽略，finish 不制造重跑", async () => {
    const guard = createInFlightGuard();
    guard.defer();
    expect(guard.finish()).toBe(false);

    let calls = 0;
    await guard.run(async () => {
      calls += 1;
    });
    expect(calls).toBe(1);
  });

  it("finish 在有排队重跑时保持占用，直到重跑结束才释放", async () => {
    const guard = createInFlightGuard();
    expect(guard.tryStart()).toBe(true);
    guard.defer();
    expect(guard.finish()).toBe(true);
    // 重跑还没做，守卫仍被占用：这时第三个调用只能排队，不能并发。
    expect(guard.isRunning()).toBe(true);
    expect(guard.tryStart()).toBe(false);
    expect(guard.finish()).toBe(false);
    expect(guard.isRunning()).toBe(false);
  });
});
