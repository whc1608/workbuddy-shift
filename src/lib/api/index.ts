/**
 * 对外入口：按域拆分的实现统一从这里再导出。
 *
 * 页面与组件始终 `import * as api from "@/lib/api"`，
 * 因此拆分不改变任何调用点。
 */

export * from "./core";
export * from "./failure";
export * from "./diagnostics";
export * from "./credential";
export * from "./accounts";
export * from "./automation";
export * from "./clients";
export * from "./daily-activity";
export * from "./sessions";
export * from "./stats";
export * from "./storage";
export * from "./settings-sync";
export * from "./update";
export * from "./settings";
export * from "./notifications";
export * from "./desktop";
