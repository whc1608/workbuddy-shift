import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import { displayName } from "@/lib/account-display";
import { type PersistedPreferenceOptions, usePersistedPreferenceState } from "@/lib/persisted-preference";
import { useSupportedTools } from "@/lib/supported-tools";
import type { AccountMeta } from "@/lib/types";
import {
  accountVariant,
  variantLabel,
  variantSupportsCheckin,
  variantSupportsTravel,
} from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";

import { DEFAULT_SORT, nextSortState, parseSort, serializeSort, sortAccounts, type SortKey, type SortState } from "../ordering";
import { useCredentialConfig } from "./use-credential-config";
import { fetchTodayCheckinMap } from "../signals/fetch-maps";
import { useCheckinSignals } from "../signals/use-checkin-signals";
import { useClientStatusSignals } from "../signals/use-client-status-signals";
import { useRateLimitSignals } from "../signals/use-rate-limit-signals";
import { useTravelSignals } from "../signals/use-travel-signals";
import { useCompactMode } from "../use-compact-mode";

/**
 * 排序偏好：`accounts.sort` = `{"key":"name","dir":"asc|desc"}`。
 *
 * 国内版 / 国际版共享同一份偏好：用户按「自己关心的账号」排序，不按档位分别记住。
 * options 必须在模块级定义（对象引用稳定），否则订阅会反复重建。
 */
const SORT_PREFERENCE: PersistedPreferenceOptions<SortState> = {
  key: "accounts.sort",
  fallback: DEFAULT_SORT,
  parse: parseSort,
  serialize: serializeSort,
  eventName: "workbuddy-shift:accounts-sort-changed",
};

/**
 * 账号页的装配层：账号库、档位、信号 hooks 与所有动作集中在这里，
 * 页面只负责组合布局，网格与弹窗组件各自消费同一份控制器，避免 30+ 个 props 反复拼装。
 *
 * 信号 hooks 的入参形状保持不变（visibleAccounts / checkinAvailable / travelAvailable），
 * 账号页与账号卡片的判定仍以同一份快照为准。
 */
export function useAccountsPage() {
  const {
    accounts,
    variant,
    setVariant,
    status,
    loading,
    error,
    fetchAll,
    deleteAccount,
    creditMap,
    creditLoadingMap,
    creditUpdatedAtMap,
    refreshingCredits,
    ensureCredits,
    refreshCredits,
  } = useAccountsStore();
  const [oauthOpen, setOauthOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [switchAccount, setSwitchAccount] = useState<AccountMeta | null>(null);
  /** 「账号信息」弹框目标账号（查看信息 / 编辑备注 / 选择显示字段）。 */
  const [infoTarget, setInfoTarget] = useState<AccountMeta | null>(null);
  /** 「账号标记」弹框目标账号（R5-04；卡片图标与「…」菜单共用）。 */
  const [markTarget, setMarkTarget] = useState<AccountMeta | null>(null);
  const [codebuddyCliSwitchingId, setCodebuddyCliSwitchingId] = useState<string | null>(null);
  /** CodeBuddy IDE 切换弹窗目标（null=关闭）；切换与可选会话复制/同步在弹窗内完成。 */
  const [codebuddyIdeSwitchAccount, setCodebuddyIdeSwitchAccount] = useState<AccountMeta | null>(null);
  /** VS Code 扩展切换弹窗目标（null=关闭）；切换与可选会话复制在弹窗内完成。 */
  const [vscodeSwitchAccount, setVscodeSwitchAccount] = useState<AccountMeta | null>(null);
  /** JetBrains 切换弹窗目标（null=关闭）；切换与目标 IDE 选择在弹窗内完成。 */
  const [jetbrainsSwitchTarget, setJetbrainsSwitchTarget] = useState<AccountMeta | null>(null);
  const [installingCodebuddyCli, setInstallingCodebuddyCli] = useState(false);
  /** 刷新按钮触发的批量签到进行中 */
  const [checkinAllRunning, setCheckinAllRunning] = useState(false);
  /** 接入/升级 CLI helper 确认框 */
  const [installConfirmOpen, setInstallConfirmOpen] = useState(false);
  /** 切换 CodeBuddy CLI 确认目标（null=关闭） */
  const [cliSwitchTarget, setCliSwitchTarget] = useState<AccountMeta | null>(null);
  /** 删除账号确认目标（null=关闭） */
  const [deleteTarget, setDeleteTarget] = useState<AccountMeta | null>(null);

  const visibleAccounts = useMemo(
    () => accounts.filter((account) => accountVariant(account) === variant),
    [accounts, variant],
  );
  const checkinAvailable = variantSupportsCheckin(variant);
  const travelAvailable = variantSupportsTravel(variant);
  const enabledTools = useSupportedTools();
  const [compact, toggleCompact] = useCompactMode();
  /** 排序偏好（localStorage `accounts.sort`）：九项单选 + 同一项再点反转。 */
  const [sort, setSort] = usePersistedPreferenceState(SORT_PREFERENCE);
  /** 点击某个维度：非当前项切到该维度（用其默认方向），当前项只翻转方向。 */
  const onSortChange = useCallback(
    (key: SortKey) => setSort(nextSortState(sort, key)),
    [sort, setSort],
  );

  const { checkinMap, setCheckinMap, excludedCheckinIds, autoCheckinSettled } = useCheckinSignals({
    visibleAccounts,
    checkinAvailable,
  });
  const { travelMap, autoTravelEnabled, loadTravelMap } = useTravelSignals({
    visibleAccounts,
    travelAvailable,
  });
  const { rateLimitMap, rateLimitBorrowedMap, rateLimitEnabled } = useRateLimitSignals();
  const {
    codebuddyCli,
    codebuddyCnIde,
    vscodeExt,
    jetbrains,
    refreshCodebuddyCliStatus,
    refreshCodebuddyCnIdeStatus,
    refreshVscodeExtStatus,
    refreshJetbrainsStatus,
  } = useClientStatusSignals({
    variant,
    accountsLength: accounts.length,
    enabledTools,
  });

  /**
   * 凭据解密设置（R6-06 收尾）：账号页只用它判断「是否要提示打开写解密」。
   * 读取是异步且失败静默的，配置未到时 `config` 为 `null`，卡片网格照常渲染。
   */
  const credentialConfig = useCredentialConfig();

  useEffect(() => {
    void fetchAll();
  }, [fetchAll]);

  // 只给尚未缓存的账号拉积分；切回首页不重复请求。点「刷新积分」才强制更新。
  useEffect(() => {
    if (!visibleAccounts.length) return;
    void ensureCredits(visibleAccounts.map((account) => account.id));
  }, [visibleAccounts, ensureCredits]);

  /** 刷新按钮文案：国际版没有签到接口，只刷新积分。 */
  const refreshCreditsLabel = checkinAvailable
    ? "刷新全部账号积分并签到（仅在签到时间段内签到；忽略已关闭自动签到的账号）"
    : "刷新全部账号积分";

  /** 导出完成提示（含安全提醒）。 */
  function onExported(count: number) {
    const text = `已导出 ${count} 个账号。文件含登录 token，等同密码，请勿上传网盘或发送给他人。`;
    toast.success("导出成功", { description: text });
  }

  /**
   * 导入完成提示：计数 + token 可能过期提醒（含加密凭据能力限制），并刷新列表。
   *
   * 「0 个成功」必须是警告而不是成功：一份全 `enc:v1:` 的第三方号池文件会整份跳过，
   * 若仍弹「导入成功」，用户会以为凭证已经在库里，然后对着账号列表找不到东西。
   */
  function onImported(result: {
    imported: number;
    skipped: number;
    overwritten: number;
    encrypted: number;
  }) {
    void fetchAll();
    if (result.imported === 0) {
      const reason =
        result.skipped > 0
          ? `跳过 ${result.skipped} 个：多为第三方加密池密文或缺少凭据。`
          : "文件里没有可导入的账号。";
      toast.warning("没有导入任何账号", {
        description: `${reason}请让对方导出「完整凭证」（明文 access_token / refresh_token）后重试。`,
      });
      return;
    }
    const overwriteText = result.overwritten > 0 ? `（覆盖 ${result.overwritten} 个）` : "";
    const encryptedText =
      result.encrypted > 0
        ? `其中 ${result.encrypted} 个为加密凭据，仅可用于切换，签到/积分不可用。`
        : "";
    const text = `已导入 ${result.imported} 个${overwriteText}，跳过 ${result.skipped} 个。${encryptedText}token 可能已过期，切换后可能需要重新登录。`;
    toast.success("导入成功", { description: text });
  }

  function onDelete(account: AccountMeta) {
    // 桌面 App（Tauri WebView）不支持 window.confirm，改用 Dialog 确认
    setDeleteTarget(account);
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    const target = deleteTarget;
    setDeleteTarget(null);
    try {
      await deleteAccount(target.id);
      toast.success("账号已删除");
    } catch (e) {
      toast.error("删除失败", { description: api.asError(e) });
    }
  }

  async function onCheckin(account: AccountMeta) {
    try {
      const res = await api.checkin(account.id);
      const label =
        res.result === "success"
          ? "签到成功"
          : res.result === "already"
            ? "今天已签到"
            : "签到失败";
      const description = `${displayName(account)}${res.error ? `：${res.error}` : ""}`;
      if (res.result === "error") toast.error(label, { description });
      else toast.success(label, { description });
      // 手动签到已完成状态核验，直接使用回执，避免为已关闭账号再触发展示查询。
      if (res.result === "success" || res.result === "already") {
        setCheckinMap((prev) => ({ ...prev, [account.id]: true }));
      }
      void fetchAll();
      // 签到成功/已签到会带来积分变动，force 刷新该账号积分
      if (res.result !== "error") void refreshCredits([account.id]);
    } catch (e) {
      toast.error("签到失败", { description: api.asError(e) });
    }
  }

  async function onRefresh(account: AccountMeta) {
    try {
      const res = await api.refreshAccountToken(account.id);
      const label = displayName(account);
      if (res.needsRelogin) {
        toast.error("Token 刷新失败", {
          description: `${label}：需重新登录${res.needsReloginReason ? `（${res.needsReloginReason}）` : ""}`,
        });
      } else if (res.refreshSkipped) {
        // 抢不到凭据写入锁时 core 直接放弃刷新（issue #202）：既没打接口也没写盘，
        // 绝不能报「已刷新」，否则用户以为令牌已续期，实际一个字节都没变。
        toast.warning("稍后重试（未写入任何凭据）", {
          description: res.refreshSkippedReason,
        });
        // 列表照旧刷新（诊断字段可能变化），但到此为止，绝不落进成功提示。
        void fetchAll();
        return;
      } else {
        toast.success("Token 已刷新", { description: label });
      }
      void fetchAll();
    } catch (e) {
      toast.error("Token 刷新失败", { description: api.asError(e) });
    }
  }

  /** 刷新附带的签到遵守账号开关与签到时间段；所有账号照常刷新积分，提示实际忽略/未到时间段数量。 */
  async function onRefreshCredits() {
    if (!visibleAccounts.length || refreshingCredits || checkinAllRunning) return;
    setCheckinAllRunning(true);
    const ids = visibleAccounts.map((account) => account.id);
    let summary = "";
    let notify = toast.success;
    let title = "积分到期情况已刷新";
    try {
      if (checkinAvailable) {
        try {
          const res = await api.checkinAll(variant, true);
          const entries = res.accounts ?? [];
          const success = entries.filter((e) => e.result === "success").length;
          const already = entries.filter((e) => e.result === "already").length;
          const failed = entries.filter((e) => e.result === "error").length;
          const inactive = entries.filter((e) => e.inactive === true || e.result === "inactive").length;
          const skipped = entries.filter((e) => e.result === "skipped" && e.reason === "auto_checkin_disabled").length;
          const outsideWindow = entries.filter((e) => e.result === "skipped" && e.reason === "outside_checkin_window").length;
          const parts: string[] = [];
          if (success > 0) parts.push(`${success} 个签到成功`);
          if (already > 0) parts.push(`${already} 个已签到`);
          if (inactive > 0) parts.push(`${inactive} 个未开放签到`);
          if (failed > 0) parts.push(`${failed} 个失败`);
          if (skipped > 0) parts.push(`已忽略 ${skipped} 个关闭自动签到的账号`);
          if (outsideWindow > 0) parts.push(`${outsideWindow} 个未到签到时间段`);
          summary = res.status === "skipped" && res.reason === "already_running"
            ? "签到任务正在进行，本次仅刷新积分"
            : parts.length > 0 ? parts.join("，") : "无账号需要签到";
          const allFailed = entries.length > 0 && failed === entries.length;
          const allSkippedOrInactive =
            res.status === "skipped" ||
            entries.length === 0 ||
            (success === 0 && already === 0 && failed === 0);
          if (allFailed) {
            notify = toast.error;
            title = "积分已刷新，签到出现错误";
          } else if (allSkippedOrInactive) {
            notify = toast.info;
          }
          // 只重查实际处理过的账号状态；被跳过的账号本次未发请求，状态保持未知。
          const next = await fetchTodayCheckinMap(
            entries.filter((e) => e.result !== "skipped").map((e) => e.accountId),
          );
          if (Object.keys(next).length > 0) {
            setCheckinMap((prev) => ({ ...prev, ...next }));
          }
        } catch (e) {
          notify = toast.error;
          title = "积分已刷新，签到出现错误";
          summary = api.asError(e);
        }
      }
      await refreshCredits(ids);
      if (autoTravelEnabled) await loadTravelMap(ids);
      notify(title, { description: summary || undefined });
    } finally {
      setCheckinAllRunning(false);
    }
  }

  function onSwitchCodebuddyCli(account: AccountMeta) {
    if (codebuddyCliSwitchingId !== null) return;
    setCliSwitchTarget(account);
  }

  async function confirmSwitchCodebuddyCli() {
    const account = cliSwitchTarget;
    if (!account || codebuddyCliSwitchingId !== null) return;
    setCliSwitchTarget(null);
    setCodebuddyCliSwitchingId(account.id);
    const toastId = toast.loading("正在切换 CodeBuddy CLI…", {
      description: `正在将默认账号设为 ${displayName(account)}`,
    });
    try {
      // 后端一律先关闭正在运行的 CLI 再写状态（closeRunningCli 入参已废弃）。
      const result = await api.switchCodebuddyCliAccount(account.id);
      await refreshCodebuddyCliStatus();
      toast.success("CodeBuddy CLI 默认账号已更新", {
        id: toastId,
        description: `${displayName(account)}：${result.message || "配置已更新"}`,
      });
    } catch (error) {
      toast.error("CodeBuddy CLI 切换失败", {
        id: toastId,
        description: api.asError(error),
      });
    } finally {
      setCodebuddyCliSwitchingId(null);
    }
  }

  function onSwitchCodebuddyCnIde(account: AccountMeta) {
    if (codebuddyIdeSwitchAccount !== null) return;
    // 国内版与国际版共用同一弹窗（关联会话 / 复制会话两个 tab），只有数据源与切换接口按档位分流；
    // 弹窗本身承担确认职责（不勾选时行为与一键切换一致），不再另设轻量确认框。
    setCodebuddyIdeSwitchAccount(account);
  }

  function onInstallCodebuddyCli() {
    // 桌面 App（Tauri WebView）不支持 window.confirm，改用 Dialog 确认
    setInstallConfirmOpen(true);
  }

  async function confirmInstallCodebuddyCli() {
    setInstallConfirmOpen(false);
    setInstallingCodebuddyCli(true);
    try {
      const result = await api.installCodebuddyCliHelper();
      toast.success("CodeBuddy CLI 接入已更新", { description: result.message });
      await refreshCodebuddyCliStatus();
    } catch (error) {
      toast.error("CodeBuddy CLI 接入失败", { description: api.asError(error) });
    } finally {
      setInstallingCodebuddyCli(false);
    }
  }

  const current = status?.current;
  /**
   * 卡片顺序：按当前排序偏好计算。
   *
   * 「最近更新」用的是 `creditUpdatedAtMap`（卡片右下「18:33 更新」的同一时间戳），
   * 「额度恢复时间」用的是限额台账里尚未恢复的最近 `resetAt`。
   */
  const { ordered: orderedAccounts, priorityAccountId } = useMemo(
    () =>
      sortAccounts(visibleAccounts, sort, {
        creditMap,
        creditLoadingMap,
        creditUpdatedAtMap,
        rateLimitMap,
      }),
    [visibleAccounts, sort, creditMap, creditLoadingMap, creditUpdatedAtMap, rateLimitMap],
  );

  return {
    // 账号库与档位
    accounts,
    visibleAccounts,
    variant,
    setVariant,
    status,
    loading,
    error,
    current,
    fetchAll,
    creditMap,
    creditLoadingMap,
    creditUpdatedAtMap,
    refreshingCredits,
    // 页面能力开关与信号
    checkinAvailable,
    travelAvailable,
    enabledTools,
    compact,
    toggleCompact,
    /** 信封账号的写解密引导（配置为 null 时网格不渲染提示）。 */
    credentialConfig,
    sort,
    onSortChange,
    checkinMap,
    excludedCheckinIds,
    autoCheckinSettled,
    travelMap,
    autoTravelEnabled,
    rateLimitMap,
    rateLimitBorrowedMap,
    rateLimitEnabled,
    clientStatus: { codebuddyCli, codebuddyCnIde, vscodeExt, jetbrains },
    codebuddyCli,
    codebuddyCnIde,
    vscodeExt,
    jetbrains,
    refreshCodebuddyCliStatus,
    refreshCodebuddyCnIdeStatus,
    refreshVscodeExtStatus,
    refreshJetbrainsStatus,
    // 派生展示值
    refreshCreditsLabel,
    accountTypeLabel: variantLabel(variant),
    demoMode: api.isDemoMode(),
    cliCurrentAccountId: codebuddyCli?.activeAccountId,
    cnIdeCurrentAccountId: codebuddyCnIde?.activeAccountId,
    vscodeExtCurrentAccountId: vscodeExt?.activeAccountId,
    jetbrainsCurrentAccountId: jetbrains?.activeAccountId,
    codebuddyUsesSettingsEnv: codebuddyCli?.authMode === "settings-env",
    orderedAccounts,
    priorityAccountId,
    // 弹窗开关
    oauthOpen,
    setOauthOpen,
    exportOpen,
    setExportOpen,
    importOpen,
    setImportOpen,
    switchAccount,
    setSwitchAccount,
    infoTarget,
    setInfoTarget,
    markTarget,
    setMarkTarget,
    codebuddyCliSwitchingId,
    codebuddyIdeSwitchAccount,
    setCodebuddyIdeSwitchAccount,
    vscodeSwitchAccount,
    setVscodeSwitchAccount,
    jetbrainsSwitchTarget,
    setJetbrainsSwitchTarget,
    installingCodebuddyCli,
    checkinAllRunning,
    installConfirmOpen,
    setInstallConfirmOpen,
    cliSwitchTarget,
    setCliSwitchTarget,
    deleteTarget,
    setDeleteTarget,
    // 动作
    onExported,
    onImported,
    onDelete,
    confirmDelete,
    onCheckin,
    onRefresh,
    onRefreshCredits,
    onSwitchCodebuddyCli,
    confirmSwitchCodebuddyCli,
    onSwitchCodebuddyCnIde,
    onInstallCodebuddyCli,
    confirmInstallCodebuddyCli,
  };
}

export type AccountsPageController = ReturnType<typeof useAccountsPage>;
