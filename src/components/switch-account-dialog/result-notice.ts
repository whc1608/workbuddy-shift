import { toast } from "sonner";

import { displayName } from "@/lib/account-display";
import { SESSION_USAGE_ATTRIBUTION } from "@/lib/session-usage-copy";
import { accountVariant, variantAppName } from "@/lib/variant";
import type {
  AccountMeta,
  Session,
  SessionLinkPreviewGroup,
  SwitchResult,
  TemporaryFileInfo,
} from "@/lib/types";

/** 临时备份残留的可读描述：优先标题，其次会话 id，最后操作 id。 */
function describeTemporaryFile(item: TemporaryFileInfo): string {
  const label = item.title || item.sessionId || item.operationId;
  return `${label}：${item.reason}`;
}

/** 同一操作只保留一条：复制 / 同步 / 恢复三个报告可能重复上报同一残留。 */
function dedupeTemporaryFiles(items: TemporaryFileInfo[]): TemporaryFileInfo[] {
  const seen = new Map<string, TemporaryFileInfo>();
  for (const item of items) {
    const key = `${item.operationId}:${item.state}`;
    const existing = seen.get(key);
    if (!existing || (existing.state === "cleanupPending" && item.state === "needsRecovery")) {
      seen.set(key, item);
    }
  }
  return [...seen.values()];
}

/**
 * 把同一条说明按出现次数汇总：后端给的是固定文案，一批复制里十个会话拿到的
 * `spaceNote` 往往一模一样。逐条拼接会让提示变成十遍重复，用户反而看不到重点；
 * 相同文案只出现一次并标出涉及几个会话。
 */
function summarizeNotes(notes: string[]): string[] {
  const counts = new Map<string, number>();
  for (const note of notes) counts.set(note, (counts.get(note) ?? 0) + 1);
  return [...counts.entries()].map(([note, count]) =>
    count > 1 ? `${note}（${count} 个会话）` : note,
  );
}

/** 收集复制结果里的某一类说明（缺字段表示该会话没有这类异常）。 */
function collectNotes<T>(items: T[], pick: (item: T) => string | undefined): string[] {
  return items
    .map(pick)
    .filter((note): note is string => Boolean(note));
}

/**
 * 「副本不含会话用量记录」提示的正文（R6-08）：后端给的逐项说明 + 归属口径。
 *
 * 复制**不再搬运历史消耗**（后端 B 方案：谁用的记在谁头上），副本的「共消耗」会显示
 * 0 或偏少。只报后端的 `auxNote` 不够——用户会以为数据丢了，所以这里一律补上
 * 「历史消耗归来源账号，副本只记录自己的新增」。
 *
 * `notes` 为空时返回空串：调用方据此不弹这条提示（没有异常就不打扰）。
 */
export function auxNoteDescription(notes: readonly string[]): string {
  if (notes.length === 0) return "";
  return [...notes, SESSION_USAGE_ATTRIBUTION].join("；");
}

interface SwitchResultContext {
  result: SwitchResult;
  account: AccountMeta;
  /** 当前账号会话列表：复制失败提示用标题而不是 id。 */
  sessions: Session[];
  /** 已勾选的关联会话：同步失败/跳过提示用标题而不是 groupId。 */
  syncGroups: SessionLinkPreviewGroup[];
  requestedCopy: boolean;
  requestedSync: boolean;
}

/**
 * 切换结果提示：成功摘要 + 复制 / 同步 / 恢复 / 临时文件残留的逐项提示。
 *
 * 提示顺序与原实现一致；失败与跳过必须显式提示，不能静默当成成功。
 */
export function notifySwitchResult({
  result: res,
  account,
  sessions,
  syncGroups,
  requestedCopy,
  requestedSync,
}: SwitchResultContext): void {
  const nickname = displayName(account);
  const parts: string[] = [];
  const copyReport = res.sessionCopy;
  const copiedCount = copyReport?.copied?.length ?? 0;
  const linkedCount = copyReport?.alreadyLinked?.length ?? 0;
  const copyErrors = copyReport?.errors ?? [];
  if (copiedCount > 0) parts.push(`已复制 ${copiedCount} 个会话`);
  if (linkedCount > 0) parts.push(`已关联 ${linkedCount} 个之前复制过的会话`);
  if (copyErrors.length > 0) parts.push(`会话复制失败 ${copyErrors.length} 个`);
  const syncReport = res.sessionSync;
  const syncedItems = syncReport?.synced ?? [];
  const skippedItems = syncReport?.skipped ?? [];
  const syncErrors = syncReport?.errors ?? [];
  if (syncedItems.length > 0) parts.push(`已同步 ${syncedItems.length} 个会话`);
  if (skippedItems.length > 0) parts.push(`跳过 ${skippedItems.length} 个会话`);
  if (syncErrors.length > 0) parts.push(`会话同步失败 ${syncErrors.length} 个`);
  if (res.backup) parts.push(`备份: ${res.backup}`);
  // 成功清理：临时备份已回收，不再展示可还原路径；待清理项单独提示，不写进成功文案。
  toast.success(`已切换至「${nickname}」`, {
    description: parts.length ? parts.join("；") : `${variantAppName(accountVariant(account))} 已重启为目标账号。`,
  });
  // 复制失败或被后端跳过时必须显式提示，不能静默当成成功。
  if (copyReport?.error) {
    toast.error("会话复制未执行", { description: copyReport.error });
  } else if (copyErrors.length > 0) {
    // 失败提示必须带会话标识（能取标题时优先标题），不是只有原因。
    const sessionLabel = (id: string) =>
      sessions.find((item) => item.id === id)?.title || id;
    toast.error("部分会话未复制", {
      description: copyErrors
        .map((item) => `${sessionLabel(item.id)}：${item.error}`)
        .join("；"),
    });
  } else if (requestedCopy && !copyReport) {
    toast.warning("会话未复制", {
      description: "没有收到复制结果：当前版本可能不支持复制会话；账号已切换，但会话没有复制。",
    });
  }
  // 复制时对源正文做过容错修复：副本与原会话不再逐字相同，必须显式告知。
  // 说明本身已区分有损/无损（「已跳过」= 丢了未写完的末行；「内容未丢失」= 仅拆行），
  // 所以这里只拼后端给的说明，不追加统一后缀——否则会把无损拆分也说成丢内容。
  const repairedNotes = (copyReport?.copied ?? [])
    .map((item) => item.sourceRepaired)
    .filter((note): note is string => Boolean(note));
  if (repairedNotes.length > 0) {
    toast.warning("源会话有损坏记录，已安全处理", {
      description: repairedNotes.join("；"),
    });
  }
  // 项目归属：后端给的是「已按目录新建分组」或「未能定位/新建分组，副本未归入任何项目」
  // 两类说明。复制本身成功，所以用 warning，不能报成 error；界面不提示等于让用户
  // 事后自己发现「会话在，项目里找不到」。
  const spaceNotes = summarizeNotes(
    collectNotes(copyReport?.copied ?? [], (item) => item.spaceNote),
  );
  if (spaceNotes.length > 0) {
    toast.warning("会话的项目分组需要留意", { description: spaceNotes.join("；") });
  }
  // 账本：目标客户端缺表/缺列时副本不含用量记录。R6-08 起复制默认就不搬历史消耗，
  // 所以这里补一句归属口径——否则用户会以为副本的「共消耗」是丢了数据。
  const auxNotes = summarizeNotes(
    collectNotes(copyReport?.copied ?? [], (item) => item.auxNote),
  );
  const auxDescription = auxNoteDescription(auxNotes);
  if (auxDescription) {
    toast.warning("副本不含会话用量记录", { description: auxDescription });
  }
  // 同步结果：成功、跳过、失败与恢复信息都要能看到，不能只显示成功数。
  const groupLabel = (groupId: string) =>
    syncGroups.find((group) => group.groupId === groupId)?.title || groupId;
  // 失败与跳过合并成一条提示：两者可能同时出现，不能只报其中一类。
  const syncIssues = [
    ...syncErrors.map(
      (item) => `${item.groupId ? groupLabel(item.groupId) : "全部会话"}：${item.error}`,
    ),
    // 预览过期/前置条件变化一律跳过并给出原因，绝不显示成已同步。
    ...skippedItems.map((item) => `${groupLabel(item.groupId)}：${item.message}`),
  ];
  if (syncIssues.length > 0) {
    if (syncErrors.length > 0) {
      toast.error("部分会话没有同步（失败或已跳过）", { description: syncIssues.join("；") });
    } else {
      toast.warning("有会话没有同步（已跳过）", { description: syncIssues.join("；") });
    }
  } else if (requestedSync && !syncReport) {
    toast.warning("会话同步未执行", {
      description: "没有收到同步结果：当前版本可能不支持同步会话，或本次切换没有重启应用。",
    });
  }
  if (syncReport?.needsRecovery) {
    toast.error("有会话同步没有完成", {
      description: "已保留操作记录与备份，下次切换会先恢复；恢复完成前不会再改动目标账号的内容。",
    });
  }
  // 未完成的会话写入：可重试项只提示，阻断项由后端直接返回错误。
  const recoveryIssues = res.sessionRecovery?.needsRecovery ?? [];
  if (recoveryIssues.length > 0) {
    toast.error("有会话操作没有完成", {
      description: recoveryIssues.map((issue) => issue.reason).join("；"),
    });
  }
  // 临时备份残留：待清理说明重试入口；待恢复说明必须保留材料并给出下一步。
  const temporaryFiles = dedupeTemporaryFiles([
    ...(copyReport?.temporaryFiles ?? []),
    ...(syncReport?.temporaryFiles ?? []),
    ...(res.sessionRecovery?.temporaryFiles ?? []),
  ]);
  const pendingFiles = temporaryFiles.filter((item) => item.state === "cleanupPending");
  const recoveryFiles = temporaryFiles.filter((item) => item.state === "needsRecovery");
  // 报告级 temporaryFiles 是权威来源；仅当旧后端没给该字段时，才回退到成功项上的 pending。
  const pendingCleanupReasons: string[] = [];
  if (temporaryFiles.length === 0) {
    for (const item of copyReport?.copied ?? []) {
      if (item.cleanupState === "pending") {
        pendingCleanupReasons.push(item.cleanupError ?? "临时文件待清理");
      }
    }
    for (const item of syncedItems) {
      if (item.cleanupState === "pending") {
        pendingCleanupReasons.push(item.cleanupError ?? "临时文件待清理");
      }
    }
  }
  if (pendingCleanupReasons.length > 0 || pendingFiles.length > 0) {
    const details = [...pendingCleanupReasons, ...pendingFiles.map(describeTemporaryFile)];
    toast.warning("会话已完成，部分临时文件待清理", {
      description: `${details.join("；")}。下次切号时会自动重试清理。`,
    });
  }
  if (recoveryFiles.length > 0) {
    toast.error("有临时备份需要确认", {
      description: `${recoveryFiles.map(describeTemporaryFile).join("；")}。已保留现场，请按提示处理后重试。`,
    });
  }
}