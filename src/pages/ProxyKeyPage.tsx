// 导入 React Hooks
import { useState, useEffect, useCallback, useMemo } from "react";
// 导入 Tauri IPC 调用
import { invoke } from "@tauri-apps/api/core";
// 导入服务商秘钥自定义 Hook 与 API
import { useProviderKeys } from "../hooks/useProviderKeys";
import {
  proxyKeyApi,
  ProxyKey,
  ProxyApiType,
  providerApi,
  Provider,
  splitModels,
  PROXY_API_TYPE_CHAT,
  PROXY_API_TYPE_ANTHROPIC,
  PROXY_API_TYPE_RESPONSES,
  AUTH_MODE_BROWSER,
  ProviderAuthMode,
} from "../api";

/**
 * 代理秘钥表单数据接口
 * @property name - 令牌名称
 * @property providerKeyId - 关联服务商秘钥 ID（必选）
 * @property model - 模型名（必选）
 * @property apiType - 接口类型："chat"=OpenAI 兼容，"anthropic"=Anthropic，"responses"=Responses（默认 "chat"）
 */
interface ProxyKeyFormData {
  name: string;
  providerKeyId: string | null;
  model: string | null;
  apiType: ProxyApiType;
}

  /**
   * 代理秘钥表单组件
   */
  function ProxyKeyForm({ initialData, onSubmit, onCancel, providerKeys, providers, error }: {
    initialData: ProxyKeyFormData | null;
    onSubmit: (data: ProxyKeyFormData) => void;
    onCancel: () => void;
    providerKeys: { id: string; name: string; providerId: string; authType?: ProviderAuthMode }[];
    providers: Provider[];
    error: string | null;
  }) {
    // 构建 providerId 到 providerName 的映射
    const providerNameMap = useMemo(() => {
      const map = new Map<string, string>();
      providers.forEach((p) => map.set(p.id, p.name));
      return map;
    }, [providers]);

    // 获取服务商秘钥的显示名称：服务商（秘钥名称）
    const getProviderKeyDisplayName = (key: { id: string; name: string; providerId: string }) => {
      const providerName = providerNameMap.get(key.providerId);
      return providerName ? `${providerName}（${key.name}）` : key.name;
    };

    const [form, setForm] = useState<ProxyKeyFormData>({
      name: initialData?.name || "",
      providerKeyId: initialData?.providerKeyId || null,
      model: initialData?.model || null,
      apiType: initialData?.apiType ?? PROXY_API_TYPE_CHAT,
    });

    const [showProviderKeyDropdown, setShowProviderKeyDropdown] = useState(false);
    const [providerKeySearch, setProviderKeySearch] = useState("");

    // 根据当前接口类型筛选兼容的服务商秘钥：
    //   chat(OpenAI 兼容)：全部
    //   anthropic：仅关联服务商配置了 Anthropic 接口地址（api_anthropic_url 非空）的秘钥
    //   responses：仅关联服务商配置了 Responses 接口地址（api_responses_url 非空）的秘钥
    const compatibleProviderKeys = useMemo(() => {
      if (form.apiType === PROXY_API_TYPE_ANTHROPIC) {
        return providerKeys.filter((key) => {
          const provider = providers.find((p) => p.id === key.providerId);
          return !!provider?.apiAnthropicUrl;
        });
      }
      if (form.apiType === PROXY_API_TYPE_RESPONSES) {
        return providerKeys.filter((key) => {
          const provider = providers.find((p) => p.id === key.providerId);
          return !!provider?.apiResponsesUrl;
        });
      }
      return providerKeys;
    }, [providerKeys, providers, form.apiType]);

    // 当前选中的服务商秘钥是否为浏览器授权（ChatGPT 令牌，仅支持 Responses 协议）
    const selectedKeyIsBrowserAuth = useMemo(() => {
      if (!form.providerKeyId) return false;
      const pk = providerKeys.find((k) => k.id === form.providerKeyId);
      return pk?.authType === AUTH_MODE_BROWSER;
    }, [providerKeys, form.providerKeyId]);

    // 过滤后的服务商秘钥列表（兼容集 ∩ 搜索关键字）
    const filteredProviderKeys = useMemo(() => {
      if (!providerKeySearch.trim()) return compatibleProviderKeys;
      const query = providerKeySearch.toLowerCase();
      return compatibleProviderKeys.filter((key) => {
        const displayName = getProviderKeyDisplayName(key);
        return displayName.toLowerCase().includes(query);
      });
    }, [compatibleProviderKeys, providerKeySearch]);

  useEffect(() => {
    setForm({
      name: initialData?.name || "",
      providerKeyId: initialData?.providerKeyId || null,
      model: initialData?.model || null,
      apiType: initialData?.apiType ?? PROXY_API_TYPE_CHAT,
    });
  }, [initialData]);

  // 切换接口类型时：若当前选中的服务商秘钥不再兼容（anthropic 模式下未配置 Anthropic 接口），清空选择与模型
  useEffect(() => {
    if (!form.providerKeyId) return;
    const stillCompatible = compatibleProviderKeys.some((k) => k.id === form.providerKeyId);
    if (!stillCompatible) {
      setForm((prev) => ({ ...prev, providerKeyId: null, model: null }));
    }
  }, [compatibleProviderKeys, form.providerKeyId]);

  // 根据当前选择的 providerKey 推导 provider，再拆分 provider.models 得到可选模型列表
  const availableModels = useMemo(() => {
    if (!form.providerKeyId) return [];
    const pk = providerKeys.find((k) => k.id === form.providerKeyId);
    if (!pk) return [];
    const provider = providers.find((p) => p.id === pk.providerId);
    return provider ? splitModels(provider.models) : [];
  }, [form.providerKeyId, providerKeys, providers]);

  // 切换 providerKey 时：若当前 model 不在新列表内则置空，否则保留；若列表非空且当前空则选首个
  useEffect(() => {
    if (form.model && availableModels.length > 0 && !availableModels.includes(form.model)) {
      setForm((prev) => ({ ...prev, model: availableModels[0] || null }));
    } else if (!form.model && availableModels.length > 0) {
      setForm((prev) => ({ ...prev, model: availableModels[0] }));
    }
  }, [availableModels]);

  const [localError, setLocalError] = useState<string | null>(null);

    const handleSubmit = (e: React.FormEvent) => {
      e.preventDefault();
      setLocalError(null);
      if (!form.providerKeyId) {
        setLocalError("请选择服务商秘钥");
        return;
      }
      if (!form.model || !form.model.trim()) {
        setLocalError("请选择模型");
        return;
      }
      onSubmit({
        name: form.name,
        providerKeyId: form.providerKeyId,
        model: form.model.trim(),
        apiType:
          form.apiType === PROXY_API_TYPE_ANTHROPIC
            ? PROXY_API_TYPE_ANTHROPIC
            : form.apiType === PROXY_API_TYPE_RESPONSES
              ? PROXY_API_TYPE_RESPONSES
              : PROXY_API_TYPE_CHAT,
      });
    };

    const handleProviderKeySelect = (keyId: string) => {
      setForm({ ...form, providerKeyId: keyId, model: null });
      setShowProviderKeyDropdown(false);
      setProviderKeySearch("");
    };

    // 点击外部关闭下拉框
    useEffect(() => {
      const handleClickOutside = (e: MouseEvent) => {
        const target = e.target as HTMLElement;
        if (showProviderKeyDropdown && !target.closest('.model-select-wrapper')) {
          setShowProviderKeyDropdown(false);
          setProviderKeySearch("");
        }
      };
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }, [showProviderKeyDropdown]);

  return (
    <form onSubmit={handleSubmit} className="token-form">
      {(error || localError) && <div className="form-error">{error || localError}</div>}
      <div className="form-group">
        <label>名称 *</label>
        <input
          type="text"
          name="name"
          value={form.name}
          onChange={(e) => setForm({ ...form, name: e.target.value })}
          required
          placeholder="输入令牌名称"
          />
        </div>
      <div className="form-group">
        <label>接口类型 *</label>
        <select
          name="apiType"
          value={form.apiType}
          onChange={(e) => setForm({ ...form, apiType: e.target.value as ProxyApiType })}
        >
          <option value={PROXY_API_TYPE_CHAT}>chat</option>
          <option value={PROXY_API_TYPE_ANTHROPIC}>anthropic</option>
          <option value={PROXY_API_TYPE_RESPONSES}>responses</option>
        </select>
        <span className="form-hint">
          Anthropic / Responses 兼容模式下，仅展示已配置对应接口地址的服务商秘钥
        </span>
      </div>
      <div className="form-group">
        <label>服务商秘钥 *</label>
        <div className="model-select-wrapper">
          <div
            className="model-select-trigger"
            onClick={() => setShowProviderKeyDropdown(!showProviderKeyDropdown)}
          >
            {form.providerKeyId && compatibleProviderKeys.find((k) => k.id === form.providerKeyId) ? (
              <div className="selected-model">
                <span className="model-name">
                  {getProviderKeyDisplayName(compatibleProviderKeys.find((k) => k.id === form.providerKeyId)!)}
                </span>
              </div>
            ) : (
              <span className="placeholder">-- 请选择服务商秘钥 --</span>
            )}
            <span className="select-arrow">▼</span>
          </div>

          {showProviderKeyDropdown && (
            <div className="model-dropdown">
              <div className="search-box">
                <input
                  type="text"
                  value={providerKeySearch}
                  onChange={(e) => setProviderKeySearch(e.target.value)}
                  placeholder="搜索服务商或秘钥名称..."
                  autoFocus
                />
              </div>
              <div className="model-list">
                {filteredProviderKeys.length === 0 ? (
                  <div className="no-results">
                    {form.apiType === PROXY_API_TYPE_ANTHROPIC
                      ? "当前接口类型下没有兼容的服务商秘钥（请先到服务商页配置 Anthropic 接口地址）"
                      : form.apiType === PROXY_API_TYPE_RESPONSES
                        ? "当前接口类型下没有兼容的服务商秘钥（请先到服务商页配置 Responses 接口地址）"
                        : "未找到匹配的服务商秘钥"}
                  </div>
                ) : (
                  filteredProviderKeys.map((key) => (
                    <div
                      key={key.id}
                      className={`model-item ${form.providerKeyId === key.id ? "selected" : ""}`}
                      onClick={() => handleProviderKeySelect(key.id)}
                    >
                      <span className="model-item-name">{getProviderKeyDisplayName(key)}</span>
                    </div>
                  ))
                )}
              </div>
            </div>
          )}
        </div>
        {selectedKeyIsBrowserAuth && (
          <span className="form-hint">
            该秘钥通过浏览器授权获取（ChatGPT 账号令牌），仅支持 Responses 协议，转发目标为 ChatGPT 后端
          </span>
        )}
      </div>
      <div className="form-group">
        <label>模型 *</label>
        <select

          name="model"
          value={form.model || ""}
          onChange={(e) => setForm({ ...form, model: e.target.value || null })}
          disabled={!form.providerKeyId || availableModels.length === 0}
          required
        >
          <option value="">
            {!form.providerKeyId
              ? "请先选择服务商秘钥"
              : availableModels.length === 0
                ? "该服务商暂无模型，请先到服务商页面添加"
                : "请选择模型"}
          </option>
          {availableModels.map((m) => (
            <option key={m} value={m}>{m}</option>
          ))}
        </select>
        {form.providerKeyId && availableModels.length === 0 && (
          <span className="form-hint">请先到「服务商」页面为该服务商添加模型</span>
        )}
      </div>
      <div className="form-actions">
        <button type="button" className="btn-secondary" onClick={onCancel}>
          取消
        </button>
        <button type="submit" className="btn-primary">
          {initialData ? "保存" : "创建"}
        </button>
      </div>
    </form>
  );
}

/**
 * 代理秘钥页面组件
 * 提供代理秘钥的 CRUD 操作界面
 */
export function ProxyKeyPage() {
  // 从自定义 Hook 获取服务商秘钥数据
  const { providerKeys, loadProviderKeys } = useProviderKeys();
  // 服务商列表（用于从 providerKey.providerId 反查 models）
  const [providers, setProviders] = useState<Provider[]>([]);
  // 代理秘钥列表状态
  const [proxyKeys, setProxyKeys] = useState<ProxyKey[]>([]);
  // 加载状态
  const [loading, setLoading] = useState(false);
  // 弹窗显示状态
  const [showModal, setShowModal] = useState(false);
  // 当前编辑的令牌
  const [editingKey, setEditingKey] = useState<ProxyKey | null>(null);
  // 表单错误信息
  const [formError, setFormError] = useState<string | null>(null);
  // 操作提示消息
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);
  // 已复制的令牌 ID
  const [copiedId, setCopiedId] = useState<string | null>(null);
  // 搜索关键词
  const [searchQuery, setSearchQuery] = useState("");
  // 错误状态
  const [error, setError] = useState<string | null>(null);
  // 确认删除的令牌 ID
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  // 确认重新生成 Key 的令牌 ID
  const [confirmRegenerateId, setConfirmRegenerateId] = useState<string | null>(null);

  /**
   * 加载服务商列表
   */
  const loadProviders = useCallback(async () => {
    try {
      const data = await providerApi.getAll();
      setProviders(data);
    } catch (e) {
      console.error("Failed to load providers:", e);
    }
  }, []);

  /**
   * 加载代理秘钥列表
   */
  const loadProxyKeys = useCallback(async () => {
    setLoading(true);
    try {
      const data = await proxyKeyApi.getAll();
      setProxyKeys(data);
      setError(null);
    } catch (e) {
      setError(`${e}`);
    } finally {
      setLoading(false);
    }
  }, []);

  /**
   * 组件挂载时加载数据
   */
  useEffect(() => {
    loadProxyKeys();
    loadProviderKeys();
    loadProviders();
  }, [loadProxyKeys, loadProviderKeys, loadProviders]);

  /**
   * 显示操作提示消息
   */
  const showMessage = (type: "success" | "error", text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  };

  /**
   * 过滤后的令牌列表
   */
  const filteredKeys = useMemo(() => {
    return proxyKeys.filter((t) => {
      if (!searchQuery.trim()) return true;
      const query = searchQuery.toLowerCase();
      return (
        t.name.toLowerCase().includes(query) ||
        (t.providerKeyName?.toLowerCase().includes(query) ?? false) ||
        (t.providerName?.toLowerCase().includes(query) ?? false) ||
        (t.model?.toLowerCase().includes(query) ?? false) ||
        t.key.toLowerCase().includes(query)
      );
    });
  }, [proxyKeys, searchQuery]);

  /**
   * 处理添加新令牌
   */
  const handleAdd = () => {
    setEditingKey(null);
    setFormError(null);
    setShowModal(true);
  };

  /**
   * 处理编辑令牌
   */
  const handleEdit = (key: ProxyKey) => {
    setEditingKey(key);
    setFormError(null);
    setShowModal(true);
  };

  /**
   * 关闭弹窗
   */
  const handleCloseModal = () => {
    setShowModal(false);
    setEditingKey(null);
    setFormError(null);
  };

  /**
   * 处理表单提交（创建或更新令牌）
   */
  const handleSubmit = async (data: ProxyKeyFormData) => {
    setFormError(null);
    try {
      if (editingKey) {
        await proxyKeyApi.update(editingKey.id, data.name, data.providerKeyId, data.model, data.apiType);
        showMessage("success", "更新成功");
      } else {
        await proxyKeyApi.create(data.name, data.providerKeyId, data.model, data.apiType);
        showMessage("success", "创建成功");
      }
      handleCloseModal();
      loadProxyKeys();
    } catch (err) {
      setFormError(`操作失败：${err}`);
    }
  };

  /**
   * 处理删除令牌
   */
  const handleDelete = (id: string) => {
    setConfirmDeleteId(id);
  };

  /**
   * 确认删除
   */
  const confirmDelete = async () => {
    if (!confirmDeleteId) return;
    try {
      await proxyKeyApi.delete(confirmDeleteId);
      showMessage("success", "删除成功");
      loadProxyKeys();
    } catch (err) {
      showMessage("error", `删除失败：${err}`);
    } finally {
      setConfirmDeleteId(null);
    }
  };

  /**
   * 取消删除
   */
  const cancelDelete = () => {
    setConfirmDeleteId(null);
  };

  /**
   * 切换令牌激活状态
   */
  const handleToggleActive = async (key: ProxyKey) => {
    try {
      await proxyKeyApi.toggleActive(key.id, !key.isActive);
      showMessage("success", key.isActive ? "已停用" : "已激活");
      loadProxyKeys();
    } catch (err) {
      showMessage("error", `操作失败：${err}`);
    }
  };

  /**
   * 复制 Key 到剪贴板
   */
  const handleCopy = async (key: ProxyKey) => {
    try {
      await invoke("write_clipboard", { text: key.key });
      setCopiedId(key.id);
      showMessage("success", "Key 已复制到剪贴板");
      setTimeout(() => setCopiedId(null), 2000);
    } catch (err) {
      showMessage("error", `复制失败：${err}`);
    }
  };

  /**
   * 处理重新生成 Key
   */
  const handleRegenerate = (id: string) => {
    setConfirmRegenerateId(id);
  };

  /**
   * 确认重新生成 Key
   */
  const confirmRegenerate = async () => {
    if (!confirmRegenerateId) return;
    try {
      await proxyKeyApi.regenerate(confirmRegenerateId);
      showMessage("success", "Key 已重新生成");
      loadProxyKeys();
    } catch (err) {
      showMessage("error", `重新生成失败：${err}`);
    } finally {
      setConfirmRegenerateId(null);
    }
  };

  /**
   * 取消重新生成 Key
   */
  const cancelRegenerate = () => {
    setConfirmRegenerateId(null);
  };

  /**
   * 渲染页面内容
   */
  return (
    <div className="token-management-page">
      <div className="page-header-wrapper">
        <div className="page-header">
          <div className="page-title">
            <h1>代理秘钥</h1>
            <span className="subtitle">管理 API 访问秘钥</span>
          </div>
          <div className="page-actions">
            <button className="btn-primary" onClick={handleAdd}>
              + 添加秘钥
            </button>
          </div>
        </div>

        <div className="search-bar">
          <div className="search-icon">🔍</div>
          <input
            type="text"
            placeholder="搜索秘钥名称、服务商秘钥、模型..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="search-input"
          />
          {searchQuery && (
            <button className="search-clear" onClick={() => setSearchQuery("")}>
              ×
            </button>
          )}
        </div>

        <div className="results-info">
          共 <span className="results-count">{filteredKeys.length}</span> 个令牌
          {searchQuery && (
            <span className="search-hint">（搜索 "{searchQuery}" 的结果）</span>
          )}
        </div>
      </div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <div className="token-list-container">
        {loading ? (
          <div className="loading">
            <div className="loading-spinner"></div>
            <span>加载中...</span>
          </div>
        ) : error ? (
          <div className="error-state">
            <div className="error-icon">❌</div>
            <span>加载失败：{error}</span>
            <button onClick={loadProxyKeys}>重试</button>
          </div>
        ) : filteredKeys.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">📭</div>
            <span>
              {searchQuery ? "未找到匹配的令牌" : "暂无令牌"}
            </span>
            {!searchQuery && <button onClick={handleAdd}>添加第一个令牌</button>}
          </div>
        ) : (
          <div className="token-list">
            <div className="list-header">
              <span className="col-name">名称</span>
              <span className="col-group">服务商（秘钥名称）/ 模型</span>
              <span className="col-token">Key</span>
              <span className="col-api-type">API 类型</span>
              <span className="col-actions">操作</span>
            </div>
            {filteredKeys.map((key) => (
              <div key={key.id} className="list-row">
                <div className="col-name">
                  <span className="name-text">{key.name}</span>
                </div>
                <div className="col-group">
                  <span className="group-text">
                    {key.providerName && key.providerKeyName
                      ? `${key.providerName}（${key.providerKeyName}）`
                      : key.providerKeyName || "—"}
                    {key.model ? ` / ${key.model}` : ""}
                  </span>
                </div>
                <div className="col-token">
                  <span className="token-text">{key.key.slice(0, 16)}...</span>
                  <button
                    className={`copy-btn ${copiedId === key.id ? "copied" : ""}`}
                    onClick={() => handleCopy(key)}
                    title="复制"
                  >
                    {copiedId === key.id ? "✓" : "📋"}
                  </button>
                </div>
                <div className="col-api-type">
                  <span className={`api-type-badge api-type-${key.apiType === PROXY_API_TYPE_ANTHROPIC ? "anthropic" : key.apiType === PROXY_API_TYPE_RESPONSES ? "responses" : "openai"}`}>
                    {key.apiType === PROXY_API_TYPE_ANTHROPIC
                      ? "anthropic"
                      : key.apiType === PROXY_API_TYPE_RESPONSES
                        ? "responses"
                        : "chat"}
                  </span>
                </div>
                <div className="col-actions">
                  <label className="toggle-switch" title={key.isActive ? "停用" : "启用"}>
                    <input
                      type="checkbox"
                      checked={key.isActive}
                      onChange={() => handleToggleActive(key)}
                      className="toggle-input"
                    />
                    <span className="toggle-track">
                      <span className="toggle-thumb"></span>
                    </span>
                  </label>
                  <button className="action-btn edit" onClick={() => handleEdit(key)} title="编辑">
                    ✏️
                  </button>
                  <button
                    className="action-btn"
                    onClick={() => handleRegenerate(key.id)}
                    title="重新生成"
                  >
                    🔄
                  </button>
                  <button
                    className="action-btn delete"
                    onClick={() => handleDelete(key.id)}
                    title="删除"
                  >
                    🗑️
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 添加/编辑弹窗 */}
      {showModal && (
<div className="modal-overlay">
            <div className="modal-content">
            <div className="modal-header">
              <h3>{editingKey ? "编辑秘钥" : "添加秘钥"}</h3>
              <button className="modal-close" onClick={handleCloseModal}>
                ×
              </button>
            </div>
            <div className="modal-form">
              {/* 无服务商秘钥提示（仅添加新令牌时显示） */}
              {providerKeys.length === 0 && !editingKey && (
                <div className="no-group-hint">
                  <div className="hint-icon">⚠️</div>
                  <p>暂无服务商秘钥，建议先添加服务商秘钥以便关联</p>
                  <button className="btn-primary" onClick={() => {
                    handleCloseModal();
                    window.location.hash = "#/token-groups";
                  }}>
                    去添加服务商秘钥
                  </button>
                </div>
              )}
              {/* 令牌表单 */}
              <ProxyKeyForm
                initialData={editingKey ? {
                  name: editingKey.name,
                  providerKeyId: editingKey.providerKeyId || null,
                  model: editingKey.model || null,
                  apiType: editingKey.apiType ?? PROXY_API_TYPE_CHAT,
                } : null}
                onSubmit={handleSubmit}
                onCancel={handleCloseModal}
                providerKeys={providerKeys.map(k => ({ id: k.id, name: k.name, providerId: k.providerId, authType: k.authType }))}
                providers={providers}
                error={formError}
              />
            </div>
          </div>
        </div>
      )}

      {/* 确认删除弹窗 */}
      {confirmDeleteId && (
<div className="modal-overlay">
            <div className="modal-content">
            <div className="modal-header">
              <h3>确认删除</h3>
              <button className="modal-close" onClick={cancelDelete}>
                ×
              </button>
            </div>
            <div className="modal-body">
              <p>确定要删除此秘钥吗？此操作无法撤销。</p>
            </div>
            <div className="modal-footer">
              <button className="btn-secondary" onClick={cancelDelete}>
                取消
              </button>
              <button className="btn-danger" onClick={confirmDelete}>
                确认删除
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 确认重新生成 Key 弹窗 */}
      {confirmRegenerateId && (
<div className="modal-overlay">
            <div className="modal-content">
            <div className="modal-header">
              <h3>确认重新生成密钥</h3>
              <button className="modal-close" onClick={cancelRegenerate}>
                ×
              </button>
            </div>
            <div className="modal-body">
              <p>确定要重新生成此密钥吗？</p>
              <p style={{ marginTop: 8, color: "var(--error)" }}>
                注意：重新生成后，旧密钥将立即失效，请确保已更新所有使用该密钥的地方。
              </p>
            </div>
            <div className="modal-footer">
              <button className="btn-secondary" onClick={cancelRegenerate}>
                取消
              </button>
              <button className="btn-danger" onClick={confirmRegenerate}>
                确认重新生成
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
  