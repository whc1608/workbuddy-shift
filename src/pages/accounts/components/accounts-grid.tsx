import { Loader2 } from "lucide-react";

import { AccountCard } from "@/components/account-card";
import {
  DecryptWriteHint,
  shouldShowDecryptWriteHint,
} from "@/components/account-card/decrypt-write-hint";
import { displayName } from "@/lib/account-display";
import type { AccountMeta, AppStatus } from "@/lib/types";
import { variantAppName, variantDownloadDomain } from "@/lib/variant";
import { cn } from "@/lib/utils";

import type { AccountsPageController } from "../hooks/use-accounts-page";

function isWorkbuddyCurrent(account: AccountMeta, current: AppStatus["current"] | undefined): boolean {
  if (!current) return false;
  return Boolean(
    (current.uid && (account.uid === current.uid || account.id === current.uid)) ||
      (current.email && account.email === current.email),
  );
}

/**
 * 账号卡片网格：加载态、空态（区分国内版 / 国际版文案）与卡片列表。
 *
 * 卡片 props 全部来自账号页控制器，保证与弹窗、工具栏共用同一份状态快照。
 */
export function AccountsGrid({ page }: { page: AccountsPageController }) {
  const {
    loading,
    visibleAccounts,
    orderedAccounts,
    variant,
    compact,
    credentialConfig,
    checkinAvailable,
    checkinMap,
    autoCheckinSettled,
    excludedCheckinIds,
    autoTravelEnabled,
    travelMap,
    rateLimitEnabled,
    rateLimitMap,
    rateLimitBorrowedMap,
    creditMap,
    creditLoadingMap,
    creditUpdatedAtMap,
    priorityAccountId,
    enabledTools,
    current,
    codebuddyCli,
    codebuddyCliSwitchingId,
    codebuddyCnIde,
    codebuddyIdeSwitchAccount,
    vscodeExt,
    vscodeSwitchAccount,
    jetbrains,
    jetbrainsSwitchTarget,
    cliCurrentAccountId,
    cnIdeCurrentAccountId,
    vscodeExtCurrentAccountId,
    jetbrainsCurrentAccountId,
  } = page;

  if (loading && visibleAccounts.length === 0) {
    return (
      <div className="flex items-center gap-2 py-16 text-sm text-muted-foreground">
        <Loader2 className="animate-spin" />
        加载账号…
      </div>
    );
  }

  if (visibleAccounts.length === 0) {
    return (
      <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
        {variant === "ai" ? (
          <>
            <p>暂无国际版账号。</p>
            <p className="mt-2 text-xs leading-5">
              点击上方「OAuth 登录」添加账号；本机已登录的账号也请一并添加，以便随时切回。
              切换前请确认本机已安装 {variantAppName(variant)}（客户端下载域名 {variantDownloadDomain(variant)}）。
            </p>
          </>
        ) : (
          "暂无账号。点击上方「OAuth 扫码添加」接入账号；本机已登录的账号也请一并添加，以便随时切回。"
        )}
      </div>
    );
  }

  /**
   * 信封账号的「打开写解密」首次引导（R6-06 收尾）。
   *
   * 只取**展示顺序里的第一张**信封账号，避免 N 张卡各弹一个相同的提示；
   * 配置未加载 / 已提示过 / 写解密已开启时 `find` 直接落空，不渲染任何东西。
   * 提示通过 `AccountCard` 的 `decryptWriteHint` 槽渲染在**该卡片内部**（头部之后、积分区
   * 之前），不占网格行、不做全页横幅。
   */
  const decryptHintAccount = orderedAccounts.find((account) =>
    shouldShowDecryptWriteHint(account, credentialConfig.config),
  );

  return (
    <div className={cn("grid min-w-0 gap-5", compact ? "grid-cols-[repeat(auto-fit,minmax(min(100%,300px),1fr))]" : "grid-cols-[repeat(auto-fit,minmax(min(100%,340px),1fr))]")}>
      {/* 不要给这个网格加 items-start：它会覆盖 Grid 默认的 stretch，让同排卡片因内容长度不同而
          高低参差。卡片内部 article 是 flex-col、内容区是 flex-1，会自动吸收差额、footer 自动贴底对齐。 */}
      {orderedAccounts.map((account) => (
        <AccountCard
          key={account.id}
          account={account}
          compact={compact}
          onDelete={page.onDelete}
          onSwitch={page.setSwitchAccount}
          onShowInfo={page.setInfoTarget}
          onShowMark={page.setMarkTarget}
          onCheckin={checkinAvailable ? page.onCheckin : undefined}
          onRefresh={page.onRefresh}
          todayCheckedIn={checkinMap[account.id]}
          autoCheckinAllowed={checkinAvailable && autoCheckinSettled ? !excludedCheckinIds.has(account.id) : undefined}
          travelStatus={autoTravelEnabled ? travelMap[account.id] : undefined}
          rateLimits={rateLimitEnabled ? rateLimitMap[account.id] : undefined}
          rateLimitBorrowed={rateLimitEnabled ? rateLimitBorrowedMap[account.id] : undefined}
          credit={creditMap[account.id]}
          creditLoading={creditLoadingMap[account.id]}
          creditUpdatedAt={creditUpdatedAtMap[account.id]}
          creditPriority={account.id === priorityAccountId}
          workbuddyActive={enabledTools.workbuddy && isWorkbuddyCurrent(account, current)}
          codebuddyCliConfigured={codebuddyCli?.configured && !codebuddyCli.migrationRequired && !codebuddyCli.syncPending && !codebuddyCli.syncInProgress}
          codebuddyCliActive={enabledTools.codebuddyCli && account.id === cliCurrentAccountId}
          codebuddyCliBusy={codebuddyCliSwitchingId !== null}
          onSwitchCodebuddyCli={page.onSwitchCodebuddyCli}
          codebuddyCliLoading={codebuddyCliSwitchingId === account.id}
          codebuddyCnIdeAvailable={Boolean(codebuddyCnIde?.installed)}
          codebuddyCnIdeActive={enabledTools.codebuddyIde && account.id === cnIdeCurrentAccountId}
          codebuddyCnIdeBusy={codebuddyIdeSwitchAccount !== null}
          onSwitchCodebuddyCnIde={page.onSwitchCodebuddyCnIde}
          vscodeExtInstalled={Boolean(vscodeExt?.installed)}
          vscodeExtExtensionInstalled={Boolean(vscodeExt?.extensionInstalled)}
          vscodeExtAvailable={Boolean(vscodeExt?.installed && vscodeExt?.extensionInstalled)}
          vscodeExtActive={enabledTools.vscodeExt && account.id === vscodeExtCurrentAccountId}
          vscodeExtBusy={vscodeSwitchAccount !== null}
          onSwitchVscodeExt={page.setVscodeSwitchAccount}
          jetbrainsInstalled={Boolean(jetbrains?.installed)}
          jetbrainsPluginInstalled={Boolean(jetbrains?.pluginInstalled)}
          jetbrainsAvailable={Boolean(jetbrains?.installed && jetbrains?.pluginInstalled)}
          jetbrainsActive={enabledTools.jetbrains && account.id === jetbrainsCurrentAccountId}
          jetbrainsBusy={jetbrainsSwitchTarget !== null}
          onSwitchJetbrains={page.setJetbrainsSwitchTarget}
          enabledTools={enabledTools}
          featuresDisabled={false}
          decryptWriteHint={
            decryptHintAccount?.id === account.id ? (
              <DecryptWriteHint
                accountName={displayName(account)}
                compact={compact}
                busy={credentialConfig.saving}
                onEnable={credentialConfig.enableWriteDecrypt}
                onDismiss={credentialConfig.dismissFirstPrompt}
              />
            ) : undefined
          }
        />
      ))}
    </div>
  );
}
