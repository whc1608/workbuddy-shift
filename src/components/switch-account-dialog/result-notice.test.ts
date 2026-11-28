import { beforeEach, describe, expect, it, vi } from "vitest";

import { SESSION_USAGE_ATTRIBUTION } from "@/lib/session-usage-copy";
import type { AccountMeta, Session, SessionCopyReport, SwitchResult } from "@/lib/types";

import { notifySwitchResult } from "./result-notice";

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  info: vi.fn(),
}));

vi.mock("sonner", () => ({ toast: toastMock }));

/** 提示逻辑只用到账号显示名，其余字段不参与断言。 */
const account = { id: "acc-target", uid: "uid-target", nickname: "目标账号" } as AccountMeta;

/** 修复提示的标题（与 `result-notice.ts` 保持一致）。 */
const REPAIR_TITLE = "源会话有损坏记录，已安全处理";

/** 一个复制成功项的最小字段集合：提示只看 copied 的长度与 sourceRepaired。 */
function copiedItem(id: string, note?: string) {
  return {
    id,
    newId: `${id}-copy`,
    groupId: `group-${id}`,
    backup: null,
    cleanupState: "cleaned" as const,
    ...(note === undefined ? {} : { sourceRepaired: note }),
  };
}

/** 构造切换结果：只填本文件断言用到的字段。 */
function switchResult(copied: { id: string; sourceRepaired?: string }[]): SwitchResult {
  return {
    ok: true,
    account: "acc-target",
    backup: null,
    sessionCopy: {
      sourceUid: "uid-source",
      targetUid: "uid-target",
      copied: copied.map((item) => copiedItem(item.id, item.sourceRepaired)),
      alreadyLinked: [],
    },
  };
}

function notify(result: SwitchResult): void {
  notifySwitchResult({
    result,
    account,
    sessions: [] as Session[],
    syncGroups: [],
    requestedCopy: true,
    requestedSync: false,
  });
}

/** 只关心复制报告的用例：包一层切换结果，避免每处重复填其它字段。 */
function notifySessionCopy(sessionCopy: SessionCopyReport | undefined): void {
  notify({ ok: true, account: "acc-target", backup: null, sessionCopy });
}

/** 某个 warning 提示（按标题匹配）是否出现过。 */
function warningWith(title: string): { description?: string } | undefined {
  return toastMock.warning.mock.calls.find((call) => call[0] === title)?.[1] as
    | { description?: string }
    | undefined;
}

describe("notifySwitchResult：源会话容错修复提示", () => {
  beforeEach(() => {
    for (const fn of Object.values(toastMock)) fn.mockReset();
  });

  it("有损修复（末行截断）：说明里带「已跳过」，不额外断言丢内容话术", () => {
    const note = "源会话末尾有 1 条不完整记录（第 1000 行），已跳过";
    notify(switchResult([{ id: "sess-1", sourceRepaired: note }]));

    const warning = warningWith(REPAIR_TITLE);
    expect(warning).toBeDefined();
    // 只拼后端说明，不追加统一后缀（无损/有损的措辞由后端分别给出）。
    expect(warning?.description).toBe(note);
    expect(warning?.description).toContain("已跳过");
    expect(warning?.description).not.toContain("内容未丢失");
    // 成功摘要与其它分支不受影响。
    expect(toastMock.success).toHaveBeenCalledTimes(1);
    expect(toastMock.error).not.toHaveBeenCalled();
  });

  it("无损修复（记录粘连）：说明里带「内容未丢失」，不得出现丢内容或统一后缀话术", () => {
    const note = "源会话第 3 行有 2 条记录粘连，已拆分为独立记录（内容未丢失）";
    notify(switchResult([{ id: "sess-1", sourceRepaired: note }]));

    const warning = warningWith(REPAIR_TITLE);
    expect(warning?.description).toBe(note);
    expect(warning?.description).toContain("内容未丢失");
    expect(warning?.description).not.toContain("已跳过");
    // 无损拆分若被追加「少一次未完成的回复」这类后缀，就是在谎报丢内容。
    expect(warning?.description).not.toContain("少一次");
    expect(warning?.description).not.toContain("未完成的回复");
  });

  it("保留完整值 + 只丢残尾：说明同时讲清「丢了残尾」与「保住几条完整记录」", () => {
    const note =
      "源会话第 2 行末尾有 1 条不完整记录，已跳过（同行的 1 条完整记录已保留）";
    notify(switchResult([{ id: "sess-1", sourceRepaired: note }]));

    const description = warningWith(REPAIR_TITLE)?.description ?? "";
    expect(description).toBe(note);
    // 丢的是「末尾 1 条不完整记录」，保住的是「同行的 1 条完整记录」——两边都必须说清。
    expect(description).toContain("1 条不完整记录");
    expect(description).toContain("已跳过");
    expect(description).toContain("同行的 1 条完整记录已保留");
    expect(description).not.toContain("内容未丢失");
  });

  it("多个会话各自修复：合并成一条提示，说明用「；」拼接且逐条可见", () => {
    const first = "源会话末尾有 1 条不完整记录（第 3 行），已跳过";
    const second = "源会话第 1 行有 2 条记录粘连，已拆分为独立记录（内容未丢失）";
    notifySessionCopy({
      copied: [copiedItem("sess-1", first), copiedItem("sess-2", second), copiedItem("sess-3")],
      alreadyLinked: [],
    });

    const warning = warningWith(REPAIR_TITLE);
    expect(warning).toBeDefined();
    expect(warning?.description).toBe(`${first}；${second}`);
    // 每个会话都要能看到，不能只报第一条。
    expect(warning?.description).toContain("第 3 行");
    expect(warning?.description).toContain("第 1 行");
  });

  it("修复与失败并存：失败提示与修复提示都要出现，互不吞掉", () => {
    const note = "源会话末尾有 1 条不完整记录（第 2 行），已跳过";
    notifySessionCopy({
      copied: [copiedItem("sess-1", note)],
      alreadyLinked: [],
      errors: [{ id: "sess-2", error: "会话内容无法验证，未复制" }],
    });

    expect(toastMock.error).toHaveBeenCalledTimes(1);
    const errorToast = toastMock.error.mock.calls[0] as [string, { description?: string }];
    expect(errorToast[0]).toBe("部分会话未复制");
    expect(errorToast[1].description).toContain("会话内容无法验证，未复制");
    expect(warningWith(REPAIR_TITLE)?.description).toBe(note);
  });

  it("有成功复制但都没有修复说明时不出现该提示", () => {
    notify(switchResult([{ id: "sess-1" }]));

    expect(warningWith(REPAIR_TITLE)).toBeUndefined();
    expect(toastMock.success).toHaveBeenCalledTimes(1);
  });

  it("没有任何成功复制项时不出现修复提示（已关联复用不会做修复）", () => {
    notifySessionCopy({
      copied: [],
      alreadyLinked: [{ id: "sess-1", sessionId: "sess-1", groupId: "group-1" }],
    });

    expect(warningWith(REPAIR_TITLE)).toBeUndefined();
    expect(toastMock.success).toHaveBeenCalledTimes(1);
  });
});

describe("notifySwitchResult：项目归属与账本说明", () => {
  const SPACE_TITLE = "会话的项目分组需要留意";
  const AUX_TITLE = "副本不含会话用量记录";

  beforeEach(() => {
    for (const fn of Object.values(toastMock)) fn.mockReset();
  });

  /** 复制成功项 + 任意降级说明（缺字段表示没有该类说明）。 */
  function itemWithNotes(
    id: string,
    notes: { sourceRepaired?: string; spaceNote?: string; auxNote?: string },
  ) {
    return {
      id,
      newId: `${id}-copy`,
      groupId: `group-${id}`,
      backup: null,
      cleanupState: "cleaned" as const,
      ...notes,
    };
  }

  it("有 spaceNote 时给出 warning，文案原样展示", () => {
    const note = "目标账号里未能定位或新建项目分组，副本未归入任何项目";
    notifySessionCopy({
      copied: [itemWithNotes("sess-1", { spaceNote: note })],
      alreadyLinked: [],
    });

    const warning = warningWith(SPACE_TITLE);
    expect(warning?.description).toBe(note);
    // 复制本身是成功的：只能是 warning，不能升级成 error 或压掉成功摘要。
    expect(toastMock.error).not.toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledTimes(1);
  });

  it("没有说明时不出现这两条提示", () => {
    notifySessionCopy({ copied: [itemWithNotes("sess-1", {})], alreadyLinked: [] });

    expect(warningWith(SPACE_TITLE)).toBeUndefined();
    expect(warningWith(AUX_TITLE)).toBeUndefined();
  });

  it("同一条说明覆盖多个会话时只出现一次并标出数量", () => {
    const note = "目标账号有多个空间绑定该目录，已按目录新建分组";
    notifySessionCopy({
      copied: [itemWithNotes("sess-1", { spaceNote: note }), itemWithNotes("sess-2", { spaceNote: note })],
      alreadyLinked: [],
    });

    expect(warningWith(SPACE_TITLE)?.description).toBe(`${note}（2 个会话）`);
  });

  it("有 auxNote 时单独提示「副本不含用量记录」，并带上归属口径（R6-08）", () => {
    const note = "目标客户端不支持会话用量记录，副本不含账本";
    notifySessionCopy({
      copied: [itemWithNotes("sess-1", { auxNote: note })],
      alreadyLinked: [],
    });

    const description = warningWith(AUX_TITLE)?.description ?? "";
    // 后端说明原样保留，**并且**补上归属口径：否则用户会以为副本丢了数据。
    expect(description).toContain(note);
    expect(description).toContain(SESSION_USAGE_ATTRIBUTION);
  });

  it("修复、项目归属、账本三类说明并存时互不覆盖", () => {
    const repaired = "源会话末尾有 1 条不完整记录，已跳过";
    const space = "目标账号里未能定位或新建项目分组，副本未归入任何项目";
    const aux = "目标客户端不支持会话用量记录，副本不含账本";
    notifySessionCopy({
      copied: [itemWithNotes("sess-1", { sourceRepaired: repaired, spaceNote: space, auxNote: aux })],
      alreadyLinked: [],
    });

    expect(warningWith(REPAIR_TITLE)?.description).toBe(repaired);
    expect(warningWith(SPACE_TITLE)?.description).toBe(space);
    expect(warningWith(AUX_TITLE)?.description).toContain(aux);
    expect(toastMock.success).toHaveBeenCalledTimes(1);
  });
});
