import { useMemo, useState, type ReactElement } from "react";

import * as api from "@/lib/api";
import { cn } from "@/lib/utils";
import { AboutCard } from "./about-card";
import { AppearanceCard } from "./appearance-card";
import { ArchiveSyncCard } from "./archive-sync-card";
import { AutoCheckinCard } from "./auto-checkin-card";
import { AutoRotateCard } from "./auto-rotate-card";
import { AutomationSyncCard } from "./automation-sync-card";
import { CompanionCard } from "./companion-card";
import { CredentialCard } from "./credential-card";
import { DiagnosticsCard } from "./diagnostics-card";
import { PendingWritesCard } from "./pending-writes-card";
import { ErrorLogCard } from "./error-log-card";
import { NotificationHistoryCard } from "./notification-history-card";
import { PermissionCheckCard } from "./permission-check-card";
import { RateLimitCard } from "./rate-limit-card";
import { SettingsBundleCard } from "./settings-bundle-card";
import { SettingsSearch } from "./settings-search";
import { SettingsSyncCard } from "./settings-sync-card";
import { SettingsToc } from "./settings-toc";
import { StartupCard } from "./startup-card";
import { SupportedToolsCard } from "./supported-tools-card";
import { UpdateCard } from "./update-card";
import { filterSections, groupSettingsSections, visibleSettingsSections } from "./sections";

/**
 * 页面主体的列定义。
 *
 * 有节时才铺 `[200px_1fr]` 双栏（左目录 + 右正文）。搜索无命中时 `groups` 为空、
 * 目录不渲染，若仍留着 `200px` 的第一列，正文会被自动放进那 200px 里（T05 缺陷 A2：
 * 空态提示被折成三行）。所以空态整块退回单列，正文按 `max-w-3xl` 居中。
 */
export function settingsBodyClass(hasSections: boolean): string {
  return hasSections
    ? "min-w-0 xl:grid xl:grid-cols-[200px_minmax(0,1fr)] xl:items-start xl:gap-10"
    : "min-w-0";
}

/**
 * 设置页。
 *
 * 信息架构（T05）：
 * - **四组**（通用 / 账号与同步 / 自动化 / 桌面与维护），组内顺序由 `sections.ts` 的
 *   注册表决定——页面正文不再手写一串卡片，避免「目录顺序」与「页面顺序」两处漂移；
 * - **宽屏目录**：`xl` 以上 `[200px_1fr]` 双栏 + sticky 目录（`SettingsToc`），
 *   支持 `/settings#settings-startup` 深链；
 * - **搜索**：索引来自注册表的标题 + 行标题 + 同义词，不读 DOM。
 *
 * 环境可见性（演示模式 / WebUI / 桌面）只在 `sections.ts` 的 `availability` 写一次，
 * 渲染与目录过滤共用同一份结果，不会出现「目录里有、页面上没有」。
 */
export default function SettingsPage() {
  const [query, setQuery] = useState("");
  /** 导入设置包后 +1：用它给各卡片换 key，强制重新挂载并重读配置。 */
  const [reloadKey, setReloadKey] = useState(0);

  const allSections = useMemo(() => visibleSettingsSections(), []);
  const matched = useMemo(() => filterSections(allSections, query), [allSections, query]);
  const groups = useMemo(() => groupSettingsSections(matched), [matched]);

  /**
   * 注册表 id → 卡片。用 `Record` 而不是 switch：漏写一节时这里为 undefined，
   * `sections.test.ts` 会断言「每个注册节都有渲染器」，不会静默渲染出一片空白。
   */
  const renderers = useMemo<Record<string, () => ReactElement>>(
    () => ({
      "settings-appearance": () => <AppearanceCard />,
      "settings-tools": () => <SupportedToolsCard />,
      "settings-archive-sync": () => <ArchiveSyncCard />,
      "settings-sync": () => <SettingsSyncCard />,
      "settings-automation-sync": () => <AutomationSyncCard />,
      "settings-permission": () => <PermissionCheckCard />,
      "settings-auto-checkin": () => <AutoCheckinCard />,
      "settings-auto-rotate": () => <AutoRotateCard />,
      "settings-rate-limit": () => <RateLimitCard />,
      "settings-companion": () => <CompanionCard />,
      "settings-startup": () => <StartupCard />,
      "settings-notifications": () => <NotificationHistoryCard />,
      "settings-error-log": () => <ErrorLogCard />,
      "settings-credential": () => <CredentialCard />,
      "settings-diagnostics": () => <DiagnosticsCard />,
      "settings-pending-writes": () => <PendingWritesCard />,
      "settings-updates": () => <UpdateCard />,
      "settings-bundle": () => (
        <SettingsBundleCard onImported={() => setReloadKey((current) => current + 1)} />
      ),
      "settings-about": () => <AboutCard />,
    }),
    [],
  );

  /** 网页版（非演示）缺少桌面专属能力：页头写一行说明，避免用户找不到入口以为是缺陷。 */
  const webuiLimited = api.isWebui() && !api.isDemoMode();

  return (
    <div className="mx-auto min-w-0 w-full max-w-5xl px-4 py-6 sm:px-6 sm:py-8">
      <header className="mb-8 flex flex-col gap-4 sm:mb-10 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <h1 className="text-2xl font-semibold tracking-tight">设置</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            自动化、账号同步、桌面行为与本机维护的配置都在这里。
          </p>
          {webuiLimited ? (
            <p className="mt-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-xs leading-5 text-muted-foreground">
              网页版不提供：会话悬浮窗、开机自启、自动更新（桌面版专属）。自动签到、派猫猫旅行与自动轮换
              由服务端后台执行，关掉网页也会继续跑。
            </p>
          ) : null}
        </div>
        <SettingsSearch
          value={query}
          onChange={setQuery}
          matched={matched.length}
          total={allSections.length}
        />
      </header>

      <div className={settingsBodyClass(groups.length > 0)}>
        {groups.length > 0 ? (
          <SettingsToc groups={groups} className="hidden xl:block" />
        ) : null}

        <div className={cn("min-w-0 max-w-3xl space-y-10", groups.length === 0 && "mx-auto")}>
          {groups.length === 0 ? (
            <div className="rounded-xl border border-border/60 bg-muted/25 px-4 py-6 text-center text-sm text-muted-foreground">
              没有匹配的设置项。换个词，或清空搜索查看全部。
            </div>
          ) : (
            groups.map(({ group, sections }) => (
              <section
                key={group.id}
                aria-labelledby={`settings-group-${group.id}`}
                className="min-w-0 space-y-6"
              >
                <h2
                  id={`settings-group-${group.id}`}
                  className="px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground/70"
                >
                  {group.title}
                </h2>
                <div className="space-y-9">
                  {sections.map((section) => {
                    const render = renderers[section.id];
                    if (!render) return null;
                    return (
                      <div key={`${section.id}-${reloadKey}`} className="min-w-0">
                        {render()}
                      </div>
                    );
                  })}
                </div>
              </section>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
