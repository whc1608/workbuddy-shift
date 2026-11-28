/**
 * 与后端（wb-shift-core / Tauri commands / HTTP 路由）返回结构对齐的类型定义。
 *
 * 按域拆分后在这里统一再导出。
 *
 * 调用点始终 `import type { X } from "@/lib/types"`，拆分不改变引用方式。
 */

export * from "./account";
export * from "./app";
export * from "./daily-activity";
export * from "./session";
export * from "./stats";
export * from "./storage";
export * from "./settings-sync";
export * from "./settings-bundle";
export * from "./clients";
export * from "./diagnostics";
export * from "./credential";
export * from "./failure";
