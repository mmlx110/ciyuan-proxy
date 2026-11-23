import { invoke } from "@tauri-apps/api/core";

// ============================ 授权方式 ============================
/** api_key=手动填写密钥；browser=浏览器授权（OAuth2 登录换取令牌） */
export type ProviderAuthMode = "api_key" | "browser";

export const AUTH_MODE_API_KEY: ProviderAuthMode = "api_key";
export const AUTH_MODE_BROWSER: ProviderAuthMode = "browser";

export const AUTH_MODE_LABEL: Record<ProviderAuthMode, string> = {
  api_key: "API 密钥",
  browser: "浏览器授权",
};

/** 服务商是否支持浏览器授权 */
export const providerSupportsBrowserAuth = (
  p: Pick<Provider, "authModes"> | null | undefined,
): boolean => !!p?.authModes?.includes(AUTH_MODE_BROWSER);

/**
 * 是否是 OpenAI 厂商。
 *
 * 目前浏览器授权只实现了 OpenAI 的 "Sign in with ChatGPT" 流程，
 * 其他厂商即使打开了 browser 授权方式也走不了这套 OAuth。
 */
export const isOpenAiProvider = (
  p: Pick<Provider, "name" | "apiUrl"> | null | undefined,
): boolean => {
  if (!p) return false;
  const haystack = `${p.name ?? ""} ${p.apiUrl ?? ""}`.toLowerCase();
  return haystack.includes("openai") || haystack.includes("chatgpt");
};

// ============================ ProviderKey ============================
// 对应 Rust providers_keys 表（原 token_groups）
export interface ProviderKeyWithStatus {
  id: string;
  name: string;
  apiKey: string;
  providerId: string;
  providerName: string;
  isActive: boolean;
  createdAt: string;
  /** 授权方式：api_key / browser */
  authType: ProviderAuthMode;
}

/** 浏览器授权结果 */
export interface OpenAiBrowserAuthResult {
  accessToken: string;
  refreshToken: string | null;
  expiresAt: number | null;
  accountId: string | null;
  email: string | null;
  authMeta: string;
}

export const providerKeyApi = {
  getAll: () => invoke<ProviderKeyWithStatus[]>("get_all_provider_keys"),
  create: (
    name: string,
    apiKey: string,
    providerId: string,
    auth?: {
      authType?: ProviderAuthMode;
      refreshToken?: string | null;
      authMeta?: string | null;
    },
  ) =>
    invoke<ProviderKeyWithStatus>("create_provider_key", {
      name,
      apiKey,
      providerId,
      authType: auth?.authType ?? null,
      refreshToken: auth?.refreshToken ?? null,
      authMeta: auth?.authMeta ?? null,
    }),
  update: (
    id: string,
    name: string,
    apiKey: string | null,
    providerId: string,
    auth?: {
      authType?: ProviderAuthMode;
      refreshToken?: string | null;
      authMeta?: string | null;
    },
  ) =>
    invoke<void>("update_provider_key", {
      id,
      name,
      apiKey,
      providerId,
      authType: auth?.authType ?? null,
      refreshToken: auth?.refreshToken ?? null,
      authMeta: auth?.authMeta ?? null,
    }),
  delete: (id: string) => invoke<void>("delete_provider_key", { id }),
  toggleActive: (id: string, isActive: boolean) =>
    invoke<void>("toggle_provider_key_active", { id, isActive }),
};

/** 浏览器授权（OAuth2）相关命令 */
export const providerAuthApi = {
  /** 打开浏览器完成 OpenAI（ChatGPT）登录授权，返回令牌 */
  startOpenAiBrowserAuth: () =>
    invoke<OpenAiBrowserAuthResult>("start_openai_browser_auth"),
  /** 取消正在等待的授权 */
  cancelOpenAiBrowserAuth: () => invoke<void>("cancel_openai_browser_auth"),
};

// ============================ ProxyKey ============================
// 对应 Rust proxy_keys 表（原 tokens）

/** 接口类型：chat=OpenAI 兼容，anthropic=Anthropic Messages，responses=OpenAI Responses */
export type ProxyApiType = "chat" | "anthropic" | "responses";

export interface ProxyKey {
  id: string;
  name: string;
  key: string;
  providerKeyId: string | null;
  providerKeyName: string | null;
  providerName: string | null;
  model: string | null;
  apiType: ProxyApiType;
  expiresAt: string | null;
  isActive: boolean;
  createdAt: string;
}

export const PROXY_API_TYPE_CHAT: ProxyApiType = "chat";
export const PROXY_API_TYPE_ANTHROPIC: ProxyApiType = "anthropic";
export const PROXY_API_TYPE_RESPONSES: ProxyApiType = "responses";

export const PROXY_API_TYPES: ProxyApiType[] = [
  PROXY_API_TYPE_CHAT,
  PROXY_API_TYPE_ANTHROPIC,
  PROXY_API_TYPE_RESPONSES,
];

/** 把外部（历史版本可能为数字 1/2/3）传入的值归一化为 ProxyApiType */
export const normalizeProxyApiType = (value: unknown): ProxyApiType => {
  if (value === PROXY_API_TYPE_ANTHROPIC || value === PROXY_API_TYPE_RESPONSES) return value;
  if (value === "2") return PROXY_API_TYPE_ANTHROPIC;
  if (value === "3") return PROXY_API_TYPE_RESPONSES;
  return PROXY_API_TYPE_CHAT;
};

export const proxyKeyApi = {
  getAll: () => invoke<ProxyKey[]>("get_all_proxy_keys"),
  create: (
    name: string,
    providerKeyId: string | null,
    model: string | null,
    apiType: ProxyApiType,
  ) =>
    invoke<ProxyKey>("create_proxy_key", { name, providerKeyId, model, apiType }),
  update: (
    id: string,
    name: string,
    providerKeyId: string | null,
    model: string | null,
    apiType: ProxyApiType,
  ) =>
    invoke<void>("update_proxy_key", { id, name, providerKeyId, model, apiType }),
  delete: (id: string) => invoke<void>("delete_proxy_key", { id }),
  toggleActive: (id: string, isActive: boolean) =>
    invoke<void>("toggle_proxy_key_active", { id, isActive }),
  regenerate: (id: string) => invoke<string>("regenerate_proxy_key", { id }),
};

// ============================ Provider ============================
export type ProviderType = 1 | 2 | 3;
export const PROVIDER_STATUS_ENABLED = 0;
export const PROVIDER_STATUS_DISABLED = 1;

export const PROVIDER_TYPE_META: Record<
  ProviderType,
  { key: "official" | "relay" | "aggregation"; label: string; description: string; cssClass: string }
> = {
  1: { key: "official", label: "官方", description: "模型厂商直连，稳定可靠", cssClass: "type-official" },
  2: { key: "relay", label: "中转", description: "第三方中转服务，价格灵活", cssClass: "type-relay" },
  3: { key: "aggregation", label: "聚合", description: "多模型聚合，统一接入", cssClass: "type-aggregation" },
};

export const getProviderTypeMeta = (t: number) =>
  PROVIDER_TYPE_META[t as ProviderType] ?? {
    key: "relay" as const,
    label: String(t),
    description: "",
    cssClass: "type-relay",
  };

export interface Provider {
  id: string;
  name: string;
  website: string | null;
  apiUrl: string;
  apiAnthropicUrl: string | null;
  apiResponsesUrl: string | null;
  description: string | null;
  models: string; // 逗号分隔的模型名列表
  type: number;
  tags: string[];
  pricing: string | null;
  status: number;
  sort: number;
  createdAt: string;
  /** 支持的授权方式：api_key / browser */
  authModes: ProviderAuthMode[];
}

export interface CreateProviderInput {
  name: string;
  website: string | null;
  apiUrl: string;
  apiAnthropicUrl: string | null;
  apiResponsesUrl: string | null;
  description: string | null;
  models: string; // 逗号分隔的模型名列表
  type: number;
  tags: string[];
  pricing: string | null;
  status: number;
  sort: number;
  /** 支持的授权方式：api_key / browser */
  authModes: ProviderAuthMode[];
}

export const isProviderEnabled = (p: Pick<Provider, "status">) => p.status === PROVIDER_STATUS_ENABLED;

export const providerApi = {
  getAll: () => invoke<Provider[]>("get_all_providers"),
  create: (input: CreateProviderInput) =>
    invoke<Provider>("create_provider", { ...input } as Record<string, unknown>),
  update: (id: string, input: CreateProviderInput) =>
    invoke<void>("update_provider", { id, ...input } as Record<string, unknown>),
  delete: (id: string) => invoke<void>("delete_provider", { id }),
  toggleStatus: (id: string, isActive: boolean) =>
    invoke<void>("toggle_provider_status", { id, isActive }),
};

// 模型名工具：拆分/合并 provider.models（逗号分隔）
export const splitModels = (models: string): string[] =>
  models
    .split(",")
    .map((m) => m.trim())
    .filter((m) => m.length > 0);

export const joinModels = (models: string[]): string =>
  models.map((m) => m.trim()).filter((m) => m.length > 0).join(",");

// ============================ Remote Provider ============================
// 从服务端 /api/provider/web/service_provider/picker 拉取
export interface RemoteProvider {
  remoteId: string;
  name: string;
  website: string | null;
  // 服务端 Jackson 配置 NON_NULL：未配置 API 地址的服务商（如自建网关）不会下发该字段
  apiUrl: string | null;
  apiAnthropicUrl: string | null;
  apiResponsesUrl: string | null;
  description: string | null;
  type: number;
  typeDesc: string | null;
  tagList: string[];
  pricing: string | null;
  sort: number;
  // Picker 接口(/picker)在每条目内附带关联模型;老接口可能没有,默认空数组
  models?: RemoteProviderModel[];
}

export interface RemoteProviderModel {
  modelName: string;
  type: number;
  typeDesc: string | null;
  contextWindow: number | null;
  aliasName: string | null;
  sort: number;
}

export interface FetchRemoteProviderOptions {
  type?: ProviderType;
  keyword?: string;
}

export const remoteProviderApi = {
  fetchList: (serverUrl: string, options: FetchRemoteProviderOptions = {}) =>
    invoke<RemoteProvider[]>("fetch_remote_providers", {
      serverUrl,
      type: options.type ?? null,
      keyword: options.keyword?.trim() ? options.keyword.trim() : null,
    }),
  fetchDetail: (serverUrl: string, providerId: string) =>
    invoke<{ models: RemoteProviderModel[] }>("fetch_remote_provider_detail", {
      serverUrl,
      providerId,
    }),
  // 拉取远端服务商关联模型，返回逗号分隔的 models 字符串，供调用方写入本地 provider.models
  syncModels: async (serverUrl: string, remoteProviderId: string) => {
    const detail = await remoteProviderApi.fetchDetail(serverUrl, remoteProviderId);
    return joinModels(detail.models.map((m) => m.modelName));
  },
};

// ============================ Feedback ============================
// 对接服务端 POST /common/web/feedback/submit
// 类型枚举与服务端 FeedbackTypeEnum 保持一致：1-Bug 2-建议 3-商务
export type FeedbackType = 1 | 2 | 3;

export const FEEDBACK_TYPE_OPTIONS: Array<{
  value: FeedbackType;
  label: string;
  description: string;
}> = [
  { value: 1, label: "Bug", description: "功能异常、报错或崩溃" },
  { value: 2, label: "建议", description: "功能优化与体验改进" },
  { value: 3, label: "商务", description: "合作咨询" },
];

export interface FeedbackSubmitInput {
  userId?: number | null;
  type: FeedbackType;
  title: string;
  content: string;
  contact?: string;
  clientVersion?: string;
  systemInfo?: string;
}

export const feedbackApi = {
  submit: (serverUrl: string, input: FeedbackSubmitInput) =>
    invoke<number>("submit_feedback", {
      serverUrl,
      input: {
        userId: input.userId ?? null,
        type: input.type,
        title: input.title,
        content: input.content,
        contact: input.contact ?? null,
        clientVersion: input.clientVersion ?? null,
        systemInfo: input.systemInfo ?? null,
      },
    }),
};

// ============================ Stats ============================
// 客户端匿名统计开关、上报与缓冲查询。服务端契约：POST {server_url}/stats/web/client/events
export interface StatsFlushReport {
  uploaded: number;
  attempted: number;
  serverReachable: boolean;
}

export const statsApi = {
  getEnabled: () => invoke<boolean>("get_stats_enabled"),
  setEnabled: (enabled: boolean) =>
    invoke<void>("set_stats_enabled", { enabled }),
  flushNow: () => invoke<StatsFlushReport>("flush_stats_now"),
  getBufferedCount: () => invoke<number>("get_stats_buffered_count"),
};
