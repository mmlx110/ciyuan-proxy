import React, { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  providerApi,
  providerAuthApi,
  Provider,
  ProviderAuthMode,
  AUTH_MODE_API_KEY,
  AUTH_MODE_BROWSER,
  isOpenAiProvider,
  providerSupportsBrowserAuth,
  OpenAiBrowserAuthResult,
} from "../api";

export interface ProviderKeyFormData {
  id?: string;
  name: string;
  apiKey: string;
  providerId: string;
  providerName?: string;
  /** 授权方式：api_key / browser */
  authType?: ProviderAuthMode;
}

interface ProviderKeyFormProps {
  initialData?: ProviderKeyFormData | null;
  onSuccess: () => void;
  onCancel: () => void;
}

/** 浏览器授权状态 */
type AuthStatus =
  | { type: "idle"; text: string }
  | { type: "loading"; text: string }
  | { type: "success"; text: string }
  | { type: "error"; text: string };

export function ProviderKeyForm({ initialData, onSuccess, onCancel }: ProviderKeyFormProps) {
  const [form, setForm] = useState<ProviderKeyFormData>({
    name: "",
    apiKey: "",
    providerId: "",
  });
  const [submitting, setSubmitting] = useState(false);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [loadingProviders, setLoadingProviders] = useState(true);
  const [showProviderDropdown, setShowProviderDropdown] = useState(false);
  const [providerSearch, setProviderSearch] = useState("");
  const [showApiKey, setShowApiKey] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authMode, setAuthMode] = useState<ProviderAuthMode>(AUTH_MODE_API_KEY);
  const [authStatus, setAuthStatus] = useState<AuthStatus>({
    type: "idle",
    text: "",
  });
  const [browserToken, setBrowserToken] = useState<OpenAiBrowserAuthResult | null>(null);
  const nameInputRef = React.useRef<HTMLInputElement>(null);
  const apiKeyInputRef = React.useRef<HTMLInputElement>(null);

  useEffect(() => {
    loadProviders();
  }, []);

  useEffect(() => {
    const handleKeyDown = async (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'c' && !e.shiftKey && !e.altKey) {
        const activeElement = document.activeElement as HTMLInputElement;
        if (activeElement && activeElement.value && activeElement.tagName === 'INPUT') {
          e.preventDefault();
          try {
            await invoke('write_clipboard', { text: activeElement.value });
          } catch {}
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'v' && !e.shiftKey && !e.altKey) {
        const activeElement = document.activeElement as HTMLInputElement;
        if (activeElement && activeElement.tagName === 'INPUT') {
          e.preventDefault();
          try {
            const text = await invoke<string>('read_clipboard');
            if (activeElement === nameInputRef.current) {
              setForm({ ...form, name: text });
            } else if (activeElement === apiKeyInputRef.current) {
              setForm({ ...form, apiKey: text });
            }
          } catch {}
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [form]);

  useEffect(() => {
    if (initialData) {
      setForm({
        name: initialData.name || "",
        apiKey: initialData.apiKey || "",
        providerId: initialData.providerId || "",
        providerName: initialData.providerName,
      });
      // 编辑已有秘钥时沿用其授权方式
      if (initialData.authType === AUTH_MODE_BROWSER) {
        setAuthMode(AUTH_MODE_BROWSER);
        setAuthStatus({
          type: "idle",
          text: "已保存浏览器授权的令牌，如需更新可重新授权。",
        });
      }
    }
  }, [initialData]);

  const loadProviders = async () => {
    try {
      const data = await providerApi.getAll();
      setProviders(data);
    } catch (err) {
      console.error("Failed to load providers:", err);
    } finally {
      setLoadingProviders(false);
    }
  };

  const filteredProviders = useMemo(() => {
    const query = providerSearch.trim().toLowerCase();
    const list = !query
      ? providers
      : providers.filter(
          (p) =>
            p.name.toLowerCase().includes(query) ||
            p.apiUrl?.toLowerCase().includes(query),
        );
    // 按服务商名称排序（中文按本地化顺序）
    return [...list].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  }, [providers, providerSearch]);

  const selectedProvider = useMemo(() => {
    return providers.find((p) => p.id === form.providerId);
  }, [providers, form.providerId]);

  /** 当前服务商是否支持浏览器授权 */
  const browserAuthSupported = providerSupportsBrowserAuth(selectedProvider);
  /** 当前服务商是否走 OpenAI(ChatGPT) 授权流程 */
  const browserAuthAvailable = browserAuthSupported && isOpenAiProvider(selectedProvider);

  const handleProviderSelect = (providerId: string, providerName: string) => {
    setForm({ ...form, providerId, providerName });
    setShowProviderDropdown(false);
    setProviderSearch("");
    // 切换服务商后重置授权态：新的服务商未必支持浏览器授权
    setBrowserToken(null);
    setAuthStatus({ type: "idle", text: "" });
    const target = providers.find((p) => p.id === providerId);
    if (!providerSupportsBrowserAuth(target)) {
      setAuthMode(AUTH_MODE_API_KEY);
    }
  };

  const handleAuthModeChange = (mode: ProviderAuthMode) => {
    setAuthMode(mode);
    setError(null);
    if (mode === AUTH_MODE_BROWSER) {
      setBrowserToken(null);
      setAuthStatus({ type: "idle", text: "" });
    }
  };

  /** 打开浏览器完成 OpenAI（ChatGPT）账号登录授权 */
  const handleBrowserAuth = async () => {
    setAuthStatus({ type: "loading", text: "正在等待浏览器授权..." });
    try {
      const result = await providerAuthApi.startOpenAiBrowserAuth();
      setBrowserToken(result);
      const account = result.email || result.accountId || "已登录账号";
      setAuthStatus({
        type: "success",
        text: `授权成功：${account}`,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setBrowserToken(null);
      setAuthStatus({ type: "error", text: `授权失败: ${msg}` });
    }
  };

  /** 取消正在等待的授权 */
  const handleCancelBrowserAuth = async () => {
    try {
      await providerAuthApi.cancelOpenAiBrowserAuth();
    } catch {
      // 取消失败不阻塞界面
    }
    setAuthStatus({ type: "idle", text: "已取消授权" });
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    if (!form.name.trim()) {
      setError("请输入名称");
      return;
    }

    if (!form.providerId.trim()) {
      setError("请选择服务商");
      return;
    }

    const isBrowserMode = authMode === AUTH_MODE_BROWSER;
    // 浏览器授权模式下，最终落库的令牌：优先用本次授权拿到的新令牌，
    // 否则沿用已有秘钥解密出来的旧令牌（编辑场景未重新授权）
    const finalApiKey = isBrowserMode
      ? browserToken?.accessToken || form.apiKey
      : form.apiKey;

    if (!finalApiKey.trim()) {
      setError(isBrowserMode ? "请先完成浏览器授权" : "请输入API密钥");
      return;
    }

    // 组装授权信息：
    //   - 浏览器授权且本次已重新授权：写入新的 refresh_token / auth_meta
    //   - 浏览器授权但未重新授权（编辑已有秘钥）：不传，保留后端已存的刷新凭据
    //   - 从浏览器授权切回 API 密钥：清空 refresh_token / auth_meta
    //   - 其余情况不传，保留后端已存的授权信息
    const authPayload = (() => {
      if (isBrowserMode) {
        if (!browserToken) return undefined;
        return {
          authType: AUTH_MODE_BROWSER as ProviderAuthMode,
          refreshToken: browserToken.refreshToken ?? null,
          authMeta: browserToken.authMeta ?? null,
        };
      }
      if (initialData?.authType === AUTH_MODE_BROWSER) {
        return {
          authType: AUTH_MODE_API_KEY as ProviderAuthMode,
          refreshToken: null,
          authMeta: null,
        };
      }
      return undefined;
    })();

    setSubmitting(true);
    try {
      if (initialData) {
        // 编辑：apiKey 变更时传入新值，未改动可传原值
        await invoke<void>("update_provider_key", {
          id: initialData.id,
          name: form.name,
          apiKey: finalApiKey,
          providerId: form.providerId,
          authType: authPayload?.authType ?? null,
          refreshToken: authPayload?.refreshToken ?? null,
          authMeta: authPayload?.authMeta ?? null,
        });
      } else {
        await invoke<void>("create_provider_key", {
          name: form.name,
          apiKey: finalApiKey,
          providerId: form.providerId,
          authType: authPayload?.authType ?? null,
          refreshToken: authPayload?.refreshToken ?? null,
          authMeta: authPayload?.authMeta ?? null,
        });
      }
      onSuccess();
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      setError(`操作失败: ${errorMsg}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <form className="token-group-form" onSubmit={handleSubmit}>
      {error && <div className="form-error">{error}</div>}
      <div className="form-row">
        <div className="form-group">
          <label>名称 *</label>
          <input
            ref={nameInputRef}
            type="text"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="如：日常翻译、代码助手"
            required
          />
        </div>
        <div className="form-group model-select-group">
          <label>服务商 *</label>
          <div className="model-select-wrapper">
            <div
              className="model-select-trigger"
              onClick={() => setShowProviderDropdown(!showProviderDropdown)}
            >
              {selectedProvider ? (
                <div className="selected-model">
                  <span className="model-name">{selectedProvider.name}</span>
                </div>
              ) : (
                <span className="placeholder">-- 选择服务商 --</span>
              )}
              <span className="select-arrow">▼</span>
            </div>

            {showProviderDropdown && (
              <div className="model-dropdown">
                <div className="search-box">
                  <input
                    type="text"
                    value={providerSearch}
                    onChange={(e) => setProviderSearch(e.target.value)}
                    placeholder="搜索服务商..."
                    autoFocus
                  />
                </div>
                <div className="model-list">
                  {loadingProviders ? (
                    <div className="loading-small">加载中...</div>
                  ) : filteredProviders.length === 0 ? (
                    <div className="no-results">未找到匹配的服务商</div>
                  ) : (
                    filteredProviders.map((p) => (
                      <div
                        key={p.id}
                        className={`model-item ${form.providerId === p.id ? "selected" : ""}`}
                        onClick={() => handleProviderSelect(p.id, p.name)}
                      >
                        <span className="model-item-name">{p.name}</span>
                        {providerSupportsBrowserAuth(p) && (
                          <span className="model-item-tag">可浏览器授权</span>
                        )}
                      </div>
                    ))
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {browserAuthSupported && (
        <div className="form-group full-width">
          <label>授权方式 *</label>
          <div className="auth-mode-switch">
            <button
              type="button"
              className={`auth-mode-option ${authMode === AUTH_MODE_API_KEY ? "active" : ""}`}
              onClick={() => handleAuthModeChange(AUTH_MODE_API_KEY)}
            >
              API 密钥
            </button>
            <button
              type="button"
              className={`auth-mode-option ${authMode === AUTH_MODE_BROWSER ? "active" : ""}`}
              onClick={() => handleAuthModeChange(AUTH_MODE_BROWSER)}
            >
              浏览器授权
            </button>
          </div>
          <p className="auth-mode-hint">
            {authMode === AUTH_MODE_BROWSER
              ? browserAuthAvailable
                ? "通过浏览器登录 ChatGPT 账号获取令牌，令牌到期后会自动续期。"
                : "当前浏览器授权流程仅支持 OpenAI（ChatGPT 账号登录）。"
              : "手动填写服务商的 API 密钥。"}
          </p>
        </div>
      )}

      {authMode === AUTH_MODE_BROWSER ? (
        <div className="form-group full-width">
          <label>浏览器授权 *</label>
          <div className="browser-auth-box">
            {authStatus.text && (
              <div className={`browser-auth-status ${authStatus.type}`}>
                {authStatus.text}
              </div>
            )}
            <div className="browser-auth-actions">
              {authStatus.type === "loading" ? (
                <>
                  <button
                    type="button"
                    className="btn-secondary"
                    onClick={handleCancelBrowserAuth}
                  >
                    取消授权
                  </button>
                  <span className="browser-auth-tip">
                    请在浏览器中完成登录，授权完成后会自动返回
                  </span>
                </>
              ) : (
                <button
                  type="button"
                  className="btn-primary"
                  onClick={handleBrowserAuth}
                  disabled={!browserAuthAvailable}
                >
                  {browserToken ? "重新授权" : "打开浏览器授权"}
                </button>
              )}
            </div>
            {browserToken && (
              <div className="browser-auth-token">
                令牌前缀：{browserToken.accessToken.slice(0, 12)}…
                {browserToken.expiresAt && (
                  <span className="browser-auth-expiry">
                    （有效期至{" "}
                    {new Date(browserToken.expiresAt * 1000).toLocaleString()}
                  </span>
                )}
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="form-group full-width">
          <label>API密钥 *</label>
          <div className="input-with-icon">
            <input
              ref={apiKeyInputRef}
              type={showApiKey ? "text" : "password"}
              value={form.apiKey}
              onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
              placeholder="sk-..."
              required
            />
            <button
              type="button"
              className="toggle-password-btn"
              onClick={() => setShowApiKey(!showApiKey)}
              title={showApiKey ? "隐藏密钥" : "显示密钥"}
            >
              {showApiKey ? "🙈" : "👁️"}
            </button>
          </div>
        </div>
      )}

      <div className="form-actions">
        <button type="submit" className="btn-primary" disabled={submitting}>
          {submitting ? "保存中..." : (initialData ? "更新" : "添加")}
        </button>
        <button type="button" className="btn-secondary" onClick={onCancel}>
          取消
        </button>
      </div>
    </form>
  );
}
