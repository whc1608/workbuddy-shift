import type {
  TokenStatsAccount,
  TokenStatsGroup,
  TokenStatsSource,
  TokenStatsTotals,
} from "@/lib/types";

/**
 * 把两份来源聚合成一份（用于「CodeBuddy IDE」标签页补齐新版日志）。
 *
 * 背景（2026-11-27 实测）：CodeBuddy IDE 升级后不再写
 * `CodeBuddyExtension/Data/**\/history/<id>/index.json`，改为用内置 CLI 内核把会话
 * 写进 `~/.codebuddy/projects/*.jsonl`——也就是本工具「CodeBuddy CLI」来源的目录。
 * 日志里每条都带 `providerData.agent = "cli"`，`ide-acp/<pid>.json` 只有 socket/pid，
 * 界面面板记录的会话 id 只覆盖 4/12，**没有任何字段能把「IDE 发起的会话」和终端
 * CLI 会话分开**。
 *
 * 所以这里是**展示层**的并集：IDE 标签页 = 旧 index.json 历史（11/26 以前）+ CLI 日志
 * （11/27 起的新版 IDE 与终端 CLI 混在一起）。两个标签页因此会有重叠数字，页头会注明。
 * 等上游给出可靠区分依据后，这段合并可以整体删掉。
 *
 * 口径与后端一致（`projection.rs` 的 `Totals::value`）：`total = input + output +
 * cacheWrite`，`uncachedInput = max(0, input - cacheRead)`，`cacheHitRate = cacheRead / input`。
 */
function addTotals(left: TokenStatsTotals, right: TokenStatsTotals): TokenStatsTotals {
  const input = left.input + right.input;
  const output = left.output + right.output;
  const cacheRead = left.cacheRead + right.cacheRead;
  const cacheWrite = left.cacheWrite + right.cacheWrite;
  return {
    total: input + output + cacheWrite,
    input,
    output,
    cacheRead,
    cacheWrite,
    uncachedInput: Math.max(0, input - cacheRead),
    records: left.records + right.records,
    cacheHitRate: input > 0 ? cacheRead / input : null,
  };
}

/** 同 key 的聚合行相加；标题 / 项目 / 会话 id / uid 取先出现的那份（后者只在缺失时补）。 */
function mergeGroups(left: TokenStatsGroup[], right: TokenStatsGroup[]): TokenStatsGroup[] {
  const merged = new Map<string, TokenStatsGroup>();
  for (const group of left) merged.set(group.key, group);
  for (const group of right) {
    const existing = merged.get(group.key);
    if (!existing) {
      merged.set(group.key, group);
      continue;
    }
    merged.set(group.key, {
      ...addTotals(existing, group),
      key: existing.key,
      title: existing.title ?? group.title,
      project: existing.project ?? group.project,
      sessionId: existing.sessionId ?? group.sessionId,
      uid: existing.uid ?? group.uid,
    });
  }
  return [...merged.values()];
}

function mergeDailyByModel(
  left: Record<string, TokenStatsGroup[]> | undefined,
  right: Record<string, TokenStatsGroup[]> | undefined,
): Record<string, TokenStatsGroup[]> | undefined {
  if (!left) return right;
  if (!right) return left;
  const merged: Record<string, TokenStatsGroup[]> = { ...left };
  for (const [model, groups] of Object.entries(right)) {
    merged[model] = merged[model] ? mergeGroups(merged[model], groups) : groups;
  }
  return merged;
}

function mergeAccounts(
  left: TokenStatsAccount[] | undefined,
  right: TokenStatsAccount[] | undefined,
): TokenStatsAccount[] | undefined {
  if (!left) return right;
  if (!right) return left;
  const merged = new Map<string, TokenStatsAccount>();
  for (const account of left) merged.set(account.uid, account);
  for (const account of right) {
    const existing = merged.get(account.uid);
    if (!existing) {
      merged.set(account.uid, account);
      continue;
    }
    merged.set(account.uid, {
      uid: existing.uid,
      summary: addTotals(existing.summary, account.summary),
      models: mergeGroups(existing.models, account.models),
      projects: mergeGroups(existing.projects, account.projects),
      daily: mergeGroups(existing.daily, account.daily),
      dailyByModel: mergeDailyByModel(existing.dailyByModel, account.dailyByModel),
      hours: mergeGroups(existing.hours, account.hours),
    });
  }
  return [...merged.values()];
}

function earliest(left: number | null | undefined, right: number | null | undefined): number | null {
  if (left == null) return right ?? null;
  if (right == null) return left;
  return Math.min(left, right);
}

function latest(left: number | null | undefined, right: number | null | undefined): number | null {
  if (left == null) return right ?? null;
  if (right == null) return left;
  return Math.max(left, right);
}

/**
 * `base` 的身份（`source` 字段）与聚合都保留，`extra` 只贡献数字。
 *
 * `requests` 按时间倒序合并：IDE 侧本来没有逐请求明细，并集后这条时间线更完整，
 * 但页面上仍只在 CLI 来源提供明细入口，这里只是保证数据不丢。
 */
export function mergeTokenSources(
  base: TokenStatsSource,
  extra: TokenStatsSource,
): TokenStatsSource {
  const requests = [...(base.requests ?? []), ...(extra.requests ?? [])].sort(
    (left, right) => right.timestamp - left.timestamp,
  );
  return {
    source: base.source,
    summary: addTotals(base.summary, extra.summary),
    models: mergeGroups(base.models, extra.models),
    projects: mergeGroups(base.projects, extra.projects),
    sessions: mergeGroups(base.sessions, extra.sessions),
    daily: mergeGroups(base.daily, extra.daily),
    dailyByModel: mergeDailyByModel(base.dailyByModel, extra.dailyByModel),
    requests,
    hours: mergeGroups(base.hours, extra.hours),
    accounts: mergeAccounts(base.accounts, extra.accounts),
    filesScanned: base.filesScanned + extra.filesScanned,
    parseErrors: base.parseErrors + extra.parseErrors,
    coverageStartAt: earliest(base.coverageStartAt, extra.coverageStartAt),
    coverageEndAt: latest(base.coverageEndAt, extra.coverageEndAt),
  };
}
