import { describe, expect, it } from "vitest";

import {
  diagnosticsFileName,
  previewPhase,
  shouldFetch,
} from "@/lib/diagnostics-format";

describe("诊断包文件名（R6-04 本地时间）", () => {
  it("固定 generatedAt + 东八区偏移得到本地时间文件名", () => {
    // 2026-11-27 16:53:36 UTC = 2026-11-28 00:53:36 +08:00
    const generatedAt = 1_795_798_416_000;
    expect(diagnosticsFileName(generatedAt, 8 * 3600)).toBe(
      "workbuddy-shift-diagnostics-2026-11-28-00-53.txt",
    );
  });

  it("UTC 偏移 0 时与 UTC 一致", () => {
    const generatedAt = 1_795_798_416_000;
    expect(diagnosticsFileName(generatedAt, 0)).toBe(
      "workbuddy-shift-diagnostics-2026-11-27-16-53.txt",
    );
  });

  it("分钟/月/日都补零", () => {
    // 2026-01-02 03:04:05 UTC
    const generatedAt = Date.UTC(2026, 0, 2, 3, 4, 5);
    expect(diagnosticsFileName(generatedAt, 0)).toBe(
      "workbuddy-shift-diagnostics-2026-01-02-03-04.txt",
    );
  });
});

describe("诊断包预览阶段（R6-10）", () => {
  it("未打开时 closed", () => {
    expect(previewPhase(false, false, false)).toBe("closed");
    expect(previewPhase(false, true, false)).toBe("closed");
  });

  it("报告未就绪但正在生成时先显示 loading", () => {
    expect(previewPhase(true, true, false)).toBe("loading");
  });

  it("已有文本优先显示 ready（重新生成时不闪白）", () => {
    expect(previewPhase(true, true, true)).toBe("ready");
    expect(previewPhase(true, false, true)).toBe("ready");
  });

  it("打开但既无文本也没在生成时为空态", () => {
    expect(previewPhase(true, false, false)).toBe("empty");
  });
});

describe("诊断包加载决策", () => {
  it("没有报告时请求（预热 / 首次预览）", () => {
    expect(shouldFetch(false, false, false)).toBe(true);
  });

  it("已有报告且不强制时复用", () => {
    expect(shouldFetch(true, false, false)).toBe(false);
  });

  it("force 会重新请求（重新生成）", () => {
    expect(shouldFetch(true, false, true)).toBe(true);
  });

  it("正在加载时不重复请求", () => {
    expect(shouldFetch(false, true, false)).toBe(false);
    expect(shouldFetch(true, true, true)).toBe(false);
  });
});
