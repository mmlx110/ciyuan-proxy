import { useState, useEffect, useMemo, useRef } from "react";
import {
  providerApi,
  remoteProviderApi,
  splitModels,
  joinModels,
  PROVIDER_TYPE_META,
  PROVIDER_STATUS_ENABLED,
  PROVIDER_STATUS_DISABLED,
  getProviderTypeMeta,
  isProviderEnabled,
  Provider,
  ProviderType,
  ProviderAuthMode,
  RemoteProvider,
  CreateProviderInput,
  AUTH_MODE_API_KEY,
  AUTH_MODE_BROWSER,
  AUTH_MODE_LABEL,
} from "../api";
import { useConfig } from "../hooks/useConfig";

const TYPE_FILTERS: { value: "all" | ProviderType; label: string }[] = [
  { value: "all", label: "全部" },
  { value: 1, label: "官方" },
  { value: 2, label: "中转" },
  { value: 3, label: "聚合" },
];

const STATUS_FILTERS: { value: "all" | "enabled" | "disabled"; label: string }[] = [
  { value: "all", label: "全部状态" },
  { value: "enabled", label: "已启用" },
  { value: "disabled", label: "已停用" },
];

export function ProviderManagementPage() {
  const { config } = useConfig();
  const [providers, setProviders] = useState<Provider[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showModal, setShowModal] = useState(false);
  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [message, setMessage] = useState<{ type: "success" | "error" | "warning"; text: string } | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState<"all" | ProviderType>("all");
  const [statusFilter, setStatusFilter] = useState<"all" | "enabled" | "disabled">("all");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  // 服务商表单中的模型编辑状态（基于 provider.models 逗号分隔字段）
  // 数组顺序 = 落库顺序（逗号分隔字符串的前后顺序）；新添加的模型插到最前
  const [providerModels, setProviderModels] = useState<string[]>([]);
  const [newModelName, setNewModelName] = useState("");
  // 模型标签拖动排序状态（Pointer 事件实现，Tauri WebView2 下 HTML5 DnD 不生效）：
  // dragModelIndex = 按下拖动的源下标，hoverModelIndex = 指针当前所在下标
  const [dragModelIndex, setDragModelIndex] = useState<number | null>(null);
  const [hoverModelIndex, setHoverModelIndex] = useState<number | null>(null);
  const dragModelSourceRef = useRef<number | null>(null);
  const hoverModelIndexRef = useRef<number | null>(null);

  const [formData, setFormData] = useState<CreateProviderInput>({
    name: "",
    website: null,
    apiUrl: "",
    apiAnthropicUrl: null,
    apiResponsesUrl: null,
    description: null,
    models: "",
    type: 1,
    tags: [],
    pricing: null,
    status: PROVIDER_STATUS_ENABLED,
    sort: 0,
    authModes: [AUTH_MODE_API_KEY],
  });
  const [tagsInput, setTagsInput] = useState("");

  // 添加/编辑表单内「从服务端选择填充」相关状态
  const [showFormRemoteModal, setShowFormRemoteModal] = useState(false);
  const [formRemoteItems, setFormRemoteItems] = useState<RemoteProvider[]>([]);
  const [formRemoteLoading, setFormRemoteLoading] = useState(false);
  const [formRemoteSelectedId, setFormRemoteSelectedId] = useState<string | null>(null);
  const [formRemoteKeyword, setFormRemoteKeyword] = useState("");
  const [formRemoteError, setFormRemoteError] = useState<string | null>(null);

  useEffect(() => {
    loadProviders();
  }, []);

  const loadProviders = async () => {
    try {
      setLoading(true);
      setError(null);
      const providersData = await providerApi.getAll();
      setProviders(providersData);
    } catch (e) {
      setError(`加载失败：${e}`);
    } finally {
      setLoading(false);
    }
  };

  // 取服务商的模型名列表（拆分 provider.models 逗号分隔）
  const getModelsByProviderId = (provider: Provider) => {
    return splitModels(provider.models);
  };

  const showMessage = (type: "success" | "error" | "warning", text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  };

  const typeCount = useMemo(() => {
    const counts: Record<ProviderType | "all", number> = { all: providers.length, 1: 0, 2: 0, 3: 0 };
    providers.forEach((p) => {
      const t = p.type as ProviderType;
      if (counts[t] !== undefined) counts[t] += 1;
    });
    return counts;
  }, [providers]);

  const filteredProviders = useMemo(() => {
    let list = providers;
    if (typeFilter !== "all") {
      list = list.filter((p) => Number(p.type) === Number(typeFilter));
    }
    if (statusFilter !== "all") {
      list = list.filter((p) =>
        statusFilter === "enabled" ? isProviderEnabled(p) : !isProviderEnabled(p),
      );
    }
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      list = list.filter(
        (p) =>
          p.name.toLowerCase().includes(q) ||
          p.apiUrl.toLowerCase().includes(q) ||
          p.apiAnthropicUrl?.toLowerCase().includes(q) ||
          p.apiResponsesUrl?.toLowerCase().includes(q) ||
          p.website?.toLowerCase().includes(q) ||
          p.tags.some((t) => t.toLowerCase().includes(q)) ||
          p.description?.toLowerCase().includes(q),
      );
    }
    // 按服务商名称排序（中文按本地化顺序）。排序在过滤之后做，避免无意义的工作。
    return [...list].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  }, [providers, typeFilter, statusFilter, searchQuery]);

  const resetFormRemotePicker = () => {
    setShowFormRemoteModal(false);
    setFormRemoteItems([]);
    setFormRemoteLoading(false);
    setFormRemoteSelectedId(null);
    setFormRemoteKeyword("");
    setFormRemoteError(null);
  };

  const openFormRemotePicker = () => {
    setShowFormRemoteModal(true);
    setFormRemoteItems([]);
    setFormRemoteSelectedId(null);
    setFormRemoteKeyword("");
    setFormRemoteError(null);
    // 打开即拉取一次
    void loadFormRemoteProviders();
  };

  const closeFormRemotePicker = () => {
    setShowFormRemoteModal(false);
  };

  // 记录从远程服务端选择的服务商信息，用于「重新同步模型」
  const [remoteProviderInfo, setRemoteProviderInfo] = useState<{ remoteId: string; name: string } | null>(null);
  const [syncingModels, setSyncingModels] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);

  const pickFormRemote = (item: RemoteProvider) => {
    const nameExists = existingProviderNames.has(item.name);
    fillFormFromRemote(item);
    setShowFormRemoteModal(false);
    if (item.remoteId) {
      // 记录远端身份，「↻ 重新同步模型」按钮依赖它来手动重拉
      setRemoteProviderInfo({ remoteId: item.remoteId, name: item.name });
      // 选择器接口返回的 item.models 已经带全部模型，直接合进 providerModels;
      // 不再发额外请求，避免无谓往返
      const remoteNames = (item.models ?? []).map((m) => m.modelName);
      setProviderModels((prev) => {
        const existing = new Set(prev);
        const merged = [...prev];
        for (const name of remoteNames) {
          if (!existing.has(name)) {
            merged.push(name);
            existing.add(name);
          }
        }
        return merged;
      });
    }
    if (nameExists) {
      showMessage("warning", `名称「${item.name}」本地已存在，请修改名称后再保存`);
    }
  };

  const syncRemoteModels = async (remoteProviderId: string) => {
    const serverUrl = config?.serverUrl?.trim();
    setSyncError(null);
    if (!serverUrl) {
      setSyncError("请先在设置页配置远程服务端地址");
      showMessage("error", "请先在设置页配置远程服务端地址");
      return;
    }
    try {
      setSyncingModels(true);
      const detail = await remoteProviderApi.fetchDetail(serverUrl, remoteProviderId);
      const remoteNames = detail.models.map((m) => m.modelName);
      // 合并：保留已有，追加远程独有
      setProviderModels((prev) => {
        const existing = new Set(prev);
        const merged = [...prev];
        for (const name of remoteNames) {
          if (!existing.has(name)) {
            merged.push(name);
            existing.add(name);
          }
        }
        return merged;
      });
      if (detail.models.length === 0) {
        setSyncError("远程服务端未配置模型");
      } else {
        setSyncError(null);
      }
      showMessage("success", `从服务端同步了 ${detail.models.length} 个模型`);
    } catch (err) {
      const msg = `同步模型失败：${err}`;
      setSyncError(msg);
      showMessage("error", msg);
    } finally {
      setSyncingModels(false);
    }
  };

  const handleAddProviderModel = () => {
    if (!newModelName.trim()) return;
    if (providerModels.includes(newModelName.trim())) {
      showMessage("warning", "该模型已存在");
      return;
    }
    // 最新添加的模型排到最前（provider.models 逗号分隔字符串的第一个）
    setProviderModels([newModelName.trim(), ...providerModels]);
    setNewModelName("");
  };

  const handleRemoveProviderModel = (modelName: string) => {
    setProviderModels(providerModels.filter(m => m !== modelName));
  };

  /** 把下标 from 处的模型移动到下标 to 处，其余模型相对顺序不变 */
  const moveProviderModel = (from: number, to: number) => {
    if (from === to) return;
    if (from < 0 || from >= providerModels.length || to < 0 || to >= providerModels.length) return;
    const next = [...providerModels];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    setProviderModels(next);
  };

  const handleModelPointerDown = (e: React.PointerEvent<HTMLSpanElement>, index: number) => {
    if (e.button !== 0) return;
    // 阻止默认行为，避免拖动时触发文本选中
    e.preventDefault();
    dragModelSourceRef.current = index;
    hoverModelIndexRef.current = index;
    setDragModelIndex(index);
    setHoverModelIndex(index);
  };

  // 拖动过程监听挂在 window 上：指针移出标签或经过标签间隙也能继续判定落点
  useEffect(() => {
    if (dragModelIndex === null) return;
    const handleMove = (e: PointerEvent) => {
      const target = document.elementFromPoint(e.clientX, e.clientY);
      const tag = target?.closest<HTMLElement>("[data-model-index]");
      const raw = tag?.dataset.modelIndex;
      const idx = raw === undefined || raw === "" ? null : Number(raw);
      hoverModelIndexRef.current = idx;
      setHoverModelIndex(idx);
    };
    const handleUp = () => {
      const from = dragModelSourceRef.current;
      const to = hoverModelIndexRef.current;
      if (from !== null && to !== null) {
        moveProviderModel(from, to);
      }
      resetModelDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [dragModelIndex]);

  const resetModelDrag = () => {
    dragModelSourceRef.current = null;
    hoverModelIndexRef.current = null;
    setDragModelIndex(null);
    setHoverModelIndex(null);
  };

  const handleKeyDownModel = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      handleAddProviderModel();
    }
  };

  const handleAdd = () => {
    setEditingProvider(null);
    setRemoteProviderInfo(null);
    setFormData({
      name: "",
      website: null,
      apiUrl: "",
      apiAnthropicUrl: null,
      apiResponsesUrl: null,
      description: null,
      models: "",
      type: 1,
      tags: [],
      pricing: null,
      status: PROVIDER_STATUS_ENABLED,
      sort: providers.length,
      authModes: [AUTH_MODE_API_KEY],
    });
    setTagsInput("");
    setProviderModels([]);
    setNewModelName("");
    resetModelDrag();
    resetFormRemotePicker();
    setShowModal(true);
  };

  const handleEdit = (provider: Provider) => {
    setEditingProvider(provider);
    setFormData({
      name: provider.name,
      website: provider.website,
      apiUrl: provider.apiUrl,
      apiAnthropicUrl: provider.apiAnthropicUrl,
      apiResponsesUrl: provider.apiResponsesUrl,
      description: provider.description,
      models: provider.models,
      type: provider.type as ProviderType,
      tags: provider.tags,
      pricing: provider.pricing,
      status: provider.status,
      sort: provider.sort,
      authModes: provider.authModes?.length ? provider.authModes : [AUTH_MODE_API_KEY],
    });
    setTagsInput(provider.tags.join(", "));
    // 加载该服务商的现有模型（拆分 provider.models 逗号分隔，数组顺序即落库顺序）
    setProviderModels(splitModels(provider.models));
    setNewModelName("");
    resetModelDrag();
    resetFormRemotePicker();
    setShowModal(true);
  };

  const handleCloseModal = () => {
    setShowModal(false);
    setEditingProvider(null);
    setTagsInput("");
    setRemoteProviderInfo(null);
    resetModelDrag();
    resetFormRemotePicker();
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    // 三个 API 地址(apiUrl / apiAnthropicUrl / apiResponsesUrl)三选一必填
    const hasApiUrl = formData.apiUrl.trim().length > 0;
    const hasAnthropicUrl = (formData.apiAnthropicUrl ?? "").trim().length > 0;
    const hasResponsesUrl = (formData.apiResponsesUrl ?? "").trim().length > 0;
    if (!hasApiUrl && !hasAnthropicUrl && !hasResponsesUrl) {
      showMessage("error", "API 基础地址、Anthropic API 地址、Responses API 地址 至少填写一个");
      return;
    }
    const tags = tagsInput
      .split(/[,，;；\n]/)
      .map((t) => t.trim())
      .filter(Boolean);
    const payload: CreateProviderInput = {
      ...formData,
      tags,
      // 模型列表写入 provider.models（逗号分隔字符串）
      models: joinModels(providerModels),
      sort: Number.isFinite(formData.sort) ? formData.sort : 0,
    };

    // 名称唯一性校验：新增时名称已存在则阻止；编辑时名称被其他服务商占用则阻止
    const trimmedName = payload.name.trim();
    if (trimmedName) {
      const conflict = providers.find(
        (p) => p.name === trimmedName && (!editingProvider || p.id !== editingProvider.id),
      );
      if (conflict) {
        showMessage("error", `名称「${trimmedName}」已存在，请修改后重试`);
        return;
      }
    }

    try {
      if (editingProvider) {
        await providerApi.update(editingProvider.id, payload);
        showMessage("success", "更新成功");
      } else {
        await providerApi.create(payload);
        showMessage("success", "添加成功");
      }

      // 模型在 pickFormRemote 时已合并到 providerModels，并随 provider.models 一起落库
      setRemoteProviderInfo(null);

      handleCloseModal();
      loadProviders();
    } catch (err) {
      showMessage("error", `操作失败：${err}`);
    }
  };

  const handleDelete = (id: string) => {
    setConfirmDeleteId(id);
  };

  const loadFormRemoteProviders = async () => {
    const serverUrl = config?.serverUrl?.trim();
    if (!serverUrl) {
      showMessage("error", "请先在设置页配置远程服务端地址");
      return;
    }
    setFormRemoteLoading(true);
    setFormRemoteError(null);
    try {
      const items = await remoteProviderApi.fetchList(serverUrl, {
        keyword: formRemoteKeyword.trim() || undefined,
      });
      setFormRemoteItems(items);
      setFormRemoteSelectedId(null);
    } catch (err) {
      setFormRemoteItems([]);
      setFormRemoteSelectedId(null);
      setFormRemoteError(`获取失败：${err}`);
    } finally {
      setFormRemoteLoading(false);
    }
  };

  const fillFormFromRemote = (item: RemoteProvider) => {
    setFormRemoteSelectedId(item.remoteId);
    const tags = item.tagList ?? [];
    setFormData({
      name: item.name,
      website: item.website ?? null,
      apiUrl: item.apiUrl ?? "",
      apiAnthropicUrl: item.apiAnthropicUrl ?? null,
      apiResponsesUrl: item.apiResponsesUrl ?? null,
      description: item.description ?? null,
      // 模型在 pickFormRemote 中合并到 providerModels，提交时再 joinModels 写回
      models: "",
      type: (item.type || 2) as ProviderType,
      tags,
      pricing: item.pricing ?? null,
      status: PROVIDER_STATUS_ENABLED,
      sort: item.sort ?? 0,
      // 服务端目录不下发授权方式，沿用当前表单设置
      authModes: formData.authModes?.length ? formData.authModes : [AUTH_MODE_API_KEY],
    });
    setTagsInput(tags.join(", "));
  };

  const existingProviderNames = useMemo(() => new Set(providers.map((p) => p.name)), [providers]);

  const renderTags = (tags: string[], limit = 3) => {
    if (!tags || tags.length === 0) return null;
    const visible = tags.slice(0, limit);
    const more = tags.length - visible.length;
    return (
      <div className="provider-card-tags">
        {visible.map((tag) => (
          <span key={tag} className="provider-card-tag">
            {tag}
          </span>
        ))}
        {more > 0 && <span className="provider-card-tag-more">+{more}</span>}
      </div>
    );
  };

  return (
    <div className="provider-management-page">
      <div className="page-header-wrapper">
        <div className="page-header">
          <div className="page-title">
            <h1>服务商管理</h1>
            <span className="subtitle">管理 AI 服务服务商（官方 · 中转 · 聚合）</span>
          </div>
          <div className="page-actions">
            <button className="btn-primary" onClick={handleAdd}>
              + 添加服务商
            </button>
          </div>
        </div>

        <div className="search-bar">
          <div className="search-icon">🔍</div>
          <input
            type="text"
            placeholder="搜索名称、官网、BaseURL、标签、描述..."
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

        <div className="provider-type-filter">
          {TYPE_FILTERS.map((f) => (
            <button
              key={f.value}
              className={`type-filter-btn ${typeFilter === f.value ? "active" : ""}`}
              onClick={() => setTypeFilter(f.value)}
            >
              {f.label}
              <span
                className={`type-filter-count ${
                  f.value !== "all" ? getProviderTypeMeta(f.value).cssClass : ""
                }`}
              >
                {typeCount[f.value]}
              </span>
            </button>
          ))}
          <div className="status-filter-spacer" />
          {STATUS_FILTERS.map((f) => (
            <button
              key={f.value}
              className={`status-filter-btn ${statusFilter === f.value ? "active" : ""}`}
              onClick={() => setStatusFilter(f.value)}
            >
              {f.label}
            </button>
          ))}
        </div>

        <div className="results-info">
          共 <span className="results-count">{filteredProviders.length}</span> 个服务商
          {searchQuery && <span className="search-hint">（搜索 "{searchQuery}" 的结果）</span>}
        </div>
      </div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <div className="provider-list-container">
        {loading ? (
          <div className="loading">
            <div className="loading-spinner"></div>
            <span>加载中...</span>
          </div>
        ) : error ? (
          <div className="error-state">
            <div className="error-icon">❌</div>
            <span>{error}</span>
            <button onClick={loadProviders}>重试</button>
          </div>
        ) : filteredProviders.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">📭</div>
            <span>
              {searchQuery || typeFilter !== "all" || statusFilter !== "all"
                ? "未找到匹配的服务商"
                : "暂无服务商配置"}
            </span>
            {!searchQuery && typeFilter === "all" && statusFilter === "all" && (
              <div className="empty-actions">
                <button onClick={handleAdd}>添加第一个服务商</button>
              </div>
            )}
          </div>
        ) : (
          <div className="provider-grid">
            {filteredProviders.map((provider) => {
              const meta = getProviderTypeMeta(provider.type);
              const enabled = isProviderEnabled(provider);
              return (
                <div
                  key={provider.id}
                  className={`provider-card ${enabled ? "" : "provider-card-disabled"}`}
                >
                  <div className="card-header">
                    <div className="provider-name">
                      <span className="provider-name-text">{provider.name}</span>
                      <span className={`provider-type-badge ${meta.cssClass} provider-type-badge--inline`}>
                        {meta.label}
                      </span>
                    </div>
                    <div className="card-header-badges">
                      {provider.authModes?.includes(AUTH_MODE_BROWSER) && (
                        <span className="provider-status-badge enabled" title="支持浏览器授权换取令牌">
                          浏览器授权
                        </span>
                      )}
                      <span className={`provider-status-badge ${enabled ? "enabled" : "disabled"}`}>
                        {enabled ? "启用" : "停用"}
                      </span>
                    </div>
                  </div>
                  <div className="card-body">
                    {provider.website ? (
                      <a
                        className="provider-website"
                        href={provider.website}
                        target="_blank"
                        rel="noreferrer"
                      >
                        🌐 {shortenUrl(provider.website)}
                      </a>
                    ) : (
                      <span className="provider-website provider-website--empty" aria-hidden="true">
                        🌐
                      </span>
                    )}
                    <div className="provider-url">
                      <span className="url-label">API 基础地址</span>
                      <code>{provider.apiUrl}</code>
                    </div>
                    {provider.apiAnthropicUrl && (
                      <div className="provider-url">
                        <span className="url-label">Anthropic API 地址</span>
                        <code>{provider.apiAnthropicUrl}</code>
                      </div>
                    )}
                    {provider.apiResponsesUrl && (
                      <div className="provider-url">
                        <span className="url-label">Responses API 地址</span>
                        <code>{provider.apiResponsesUrl}</code>
                      </div>
                    )}
                    {renderTags(provider.tags)}
                    {provider.description && (
                      <div className="provider-description">{provider.description}</div>
                    )}
                  </div>
                  <div className="card-footer">
                    <div className="card-footer-meta">
                      {provider.pricing && (
                        <span className="card-pricing" title="计费模式">
                          💰 {provider.pricing}
                        </span>
                      )}
                      {/* 排序权重 #{provider.sort} 已在卡片底部隐藏,不向用户展示 */}
                      <span
                        className="card-models-count clickable"
                        title="点击编辑服务商"
                        onClick={() => handleEdit(provider)}
                      >
                        🧩 {getModelsByProviderId(provider).length} 个模型
                      </span>
                    </div>
                    <div className="card-actions">
                      <button
                        className="action-btn edit"
                        onClick={() => handleEdit(provider)}
                      >
                        ✏️ 编辑
                      </button>
                      <button
                        className="action-btn delete"
                        onClick={() => handleDelete(provider.id)}
                      >
                        🗑️
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {showModal && (
        <div className="modal-overlay">
          <div className="modal-content large">
            <div className="modal-header">
              <h3>{editingProvider ? "编辑服务商" : "添加服务商"}</h3>
              <button className="modal-close" onClick={handleCloseModal}>
                ×
              </button>
            </div>
            <form className="modal-form" onSubmit={handleSubmit}>
              <div className="form-row">
                <div className="form-group" style={{ flex: 1 }}>
                  <label>服务商名称 *</label>
                  <div className="provider-name-input-row">
                    <input
                      type="text"
                      value={formData.name}
                      onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                      placeholder="如：OpenAI、Anthropic"
                      required
                      style={{ flex: 1 }}
                    />
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={openFormRemotePicker}
                      disabled={!config?.serverUrl}
                      title={!config?.serverUrl ? "请先在设置页配置远程服务端地址" : "从远程服务端选择服务商以填充表单"}
                      style={{ marginLeft: 8 }}
                    >
                      从服务端选择
                    </button>
                  </div>
                </div>
                <div className="form-group">
                  <label>服务商类型 *</label>
                  <select
                    className={`provider-type-select ${getProviderTypeMeta(formData.type).cssClass}`}
                    value={formData.type}
                    onChange={(e) =>
                      setFormData({ ...formData, type: Number(e.target.value) as ProviderType })
                    }
                  >
                    {[1, 2, 3].map((t) => (
                      <option key={t} value={t}>
                        {PROVIDER_TYPE_META[t as ProviderType].label} - {PROVIDER_TYPE_META[t as ProviderType].description}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="form-row">
                <div className="form-group" style={{ flex: 2 }}>
                  <label>Chat Api 地址</label>
                  <input
                    type="text"
                    value={formData.apiUrl}
                    onChange={(e) => setFormData({ ...formData, apiUrl: e.target.value })}
                    placeholder="如：https://api.openai.com/v1"
                  />
                </div>
                <div className="form-group" style={{ flex: 1 }}>
                  <label>Responses API 地址</label>
                  <input
                    type="text"
                    value={formData.apiResponsesUrl || ""}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        apiResponsesUrl: e.target.value.trim() || null,
                      })
                    }
                    placeholder="如：https://api.openai.com/v1（留空则不支持）"
                  />
                  <span className="form-hint">Responses 协议请求地址；留空则不支持</span>
                </div>
              </div>

              <div className="form-row">
                <div className="form-group" style={{ flex: 1 }}>
                  <label>Anthropic API 地址</label>
                  <input
                    type="text"
                    value={formData.apiAnthropicUrl || ""}
                    onChange={(e) =>
                      setFormData({
                        ...formData,
                        apiAnthropicUrl: e.target.value.trim() || null,
                      })
                    }
                    placeholder="如：https://api.anthropic.com/v1（留空则不支持）"
                  />
                  <span className="form-hint">Anthropic 协议请求地址；留空则不支持</span>
                </div>
                <div className="form-group">
                  <label>官网地址</label>
                  <input
                    type="text"
                    value={formData.website || ""}
                    onChange={(e) =>
                      setFormData({ ...formData, website: e.target.value.trim() || null })
                    }
                    placeholder="https://openai.com"
                  />
                </div>
              </div>

              <div className="form-row">
                <div className="form-group full-width">
                  <label>授权方式</label>
                  <div className="auth-mode-switch">
                    {([AUTH_MODE_API_KEY, AUTH_MODE_BROWSER] as ProviderAuthMode[]).map((mode) => {
                      const active = (formData.authModes ?? []).includes(mode);
                      return (
                        <button
                          type="button"
                          key={mode}
                          className={`auth-mode-option ${active ? "active" : ""}`}
                          onClick={() => {
                            const current = formData.authModes ?? [AUTH_MODE_API_KEY];
                            const next = active
                              ? current.filter((m) => m !== mode)
                              : [...current, mode];
                            setFormData({
                              ...formData,
                              authModes: next.length ? next : [AUTH_MODE_API_KEY],
                            });
                          }}
                        >
                          {AUTH_MODE_LABEL[mode]}
                        </button>
                      );
                    })}
                  </div>
                  <span className="form-hint">
                    勾选「浏览器授权」后，添加服务商秘钥时可用浏览器登录换取令牌（当前仅支持 OpenAI 的 ChatGPT 账号登录）
                  </span>
                </div>
              </div>

              <div className="form-row">
                <div className="form-group" style={{ flex: 2 }}>
                  <label>核心特性标签</label>
                  <input
                    type="text"
                    value={tagsInput}
                    onChange={(e) => setTagsInput(e.target.value)}
                    placeholder="使用逗号分隔，例如：GPT-4, Claude, 多模型"
                  />
                  <span className="form-hint">
                    用于快速描述该服务商支持的能力，保存后将作为 chips 展示
                  </span>
                </div>
                <div className="form-group">
                  <label>启用状态</label>
                  <select
                    value={formData.status}
                    onChange={(e) =>
                      setFormData({ ...formData, status: parseInt(e.target.value, 10) })
                    }
                  >
                    <option value={PROVIDER_STATUS_ENABLED}>启用</option>
                    <option value={PROVIDER_STATUS_DISABLED}>停用</option>
                  </select>
                </div>
              </div>

              <div className="form-row">
                <div className="form-group full-width">
                  <label>描述</label>
                  <textarea
                    value={formData.description || ""}
                    onChange={(e) =>
                      setFormData({ ...formData, description: e.target.value.trim() || null })
                    }
                    placeholder="服务商介绍..."
                    rows={3}
                  />
                </div>
              </div>

              <div className="form-row">
                <div className="form-group full-width">
                  <label>模型列表</label>
                  <div className="model-editor">
                    <div className="model-input-row">
                      <input
                        type="text"
                        value={newModelName}
                        onChange={(e) => setNewModelName(e.target.value)}
                        onKeyDown={handleKeyDownModel}
                        placeholder="输入模型名称，按回车添加"
                        className="model-name-input"
                      />
                      <button
                        type="button"
                        className="btn-secondary btn-sm"
                        onClick={handleAddProviderModel}
                        disabled={!newModelName.trim()}
                      >
                        添加
                      </button>
                      {remoteProviderInfo && (
                        <button
                          type="button"
                          className="btn-secondary btn-sm"
                          onClick={() => syncRemoteModels(remoteProviderInfo.remoteId)}
                          disabled={syncingModels || !config?.serverUrl}
                          title="从服务端重新拉取模型"
                        >
                          {syncingModels ? "同步中..." : "↻ 重新同步模型"}
                        </button>
                      )}
                    </div>
                    {syncError && (
                      <div className="empty-models-hint sync-error">{syncError}</div>
                    )}
                    {syncingModels ? (
                      <div className="empty-models-hint">正在从服务端同步模型...</div>
                    ) : providerModels.length > 0 ? (
                      <div
                        className={`model-tags ${dragModelIndex !== null ? "model-tags--dragging" : ""}`}
                      >
                        {providerModels.map((modelName, index) => (
                          <span
                            key={modelName}
                            data-model-index={index}
                            className={[
                              "model-tag",
                              "model-tag--draggable",
                              dragModelIndex === index ? "model-tag--dragging" : "",
                              hoverModelIndex === index && dragModelIndex !== index
                                ? "model-tag--dragover"
                                : "",
                            ]
                              .filter(Boolean)
                              .join(" ")}
                            onPointerDown={(e) => handleModelPointerDown(e, index)}
                            title="按住拖动可调整顺序（第一个为默认模型）"
                          >
                            <span className="model-tag-handle" aria-hidden="true">
                              ⠿
                            </span>
                            {modelName}
                            <button
                              type="button"
                              className="model-tag-remove"
                              onPointerDown={(e) => e.stopPropagation()}
                              onClick={() => handleRemoveProviderModel(modelName)}
                              title="删除"
                            >
                              ×
                            </button>
                          </span>
                        ))}
                      </div>
                    ) : (
                      <div className="empty-models-hint">
                        暂无模型，可手动添加或从服务端同步
                      </div>
                    )}
                  </div>
                  <span className="form-hint">
                    共 {providerModels.length} 个模型；新添加的排在最前，按住标签拖动可调整顺序，第一个为默认模型
                  </span>
                </div>
              </div>

              <div className="form-actions">
                <button type="submit" className="btn-primary">
                  {editingProvider ? "更新服务商" : "添加服务商"}
                </button>
                <button type="button" className="btn-secondary" onClick={handleCloseModal}>
                  取消
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {confirmDeleteId && (
        <div className="modal-overlay">
          <div className="modal-content">
            <div className="modal-header">
              <h3>确认删除</h3>
              <button className="modal-close" onClick={() => setConfirmDeleteId(null)}>
                ×
              </button>
            </div>
            <div className="modal-body">
              <p>确定要删除此服务商吗？此操作无法撤销。</p>
            </div>
            <div className="modal-footer">
              <button className="btn-secondary" onClick={() => setConfirmDeleteId(null)}>
                取消
              </button>
              <button
                className="btn-danger"
                onClick={() => {
                  const id = confirmDeleteId;
                  setConfirmDeleteId(null);
                  void (async () => {
                    try {
                      await providerApi.delete(id);
                      showMessage("success", "删除成功");
                      loadProviders();
                    } catch (err) {
                      showMessage("error", `删除失败: ${err}`);
                    }
                  })();
                }}
              >
                确认删除
              </button>
            </div>
          </div>
        </div>
      )}

      {showFormRemoteModal && (
        <div className="modal-overlay modal-overlay--center">
          <div className="modal-content">
            <div className="modal-header">
              <h3>从服务端选择服务商</h3>
              <button className="modal-close" onClick={closeFormRemotePicker}>
                ×
              </button>
            </div>
            <div className="modal-body remote-provider-body">
              <div className="remote-fetch-section">
                <div className="remote-fetch-row">
                  <input
                    type="text"
                    className="remote-keyword-input"
                    placeholder="按名称模糊搜索（可选，回车拉取）"
                    value={formRemoteKeyword}
                    onChange={(e) => setFormRemoteKeyword(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        loadFormRemoteProviders();
                      }
                    }}
                  />
                  <button
                    className="btn-primary"
                    onClick={loadFormRemoteProviders}
                    disabled={formRemoteLoading || !config?.serverUrl}
                  >
                    {formRemoteLoading
                      ? "查询中..."
                      : formRemoteItems.length > 0
                        ? "重新查询"
                        : "查询"}
                  </button>
                </div>
              </div>

              <div className="remote-provider-list">
                {formRemoteLoading ? (
                  <div className="loading">查询中...</div>
                ) : formRemoteError ? (
                  <div className="remote-provider-error">
                    <div className="remote-provider-error-icon">⚠️</div>
                    <div className="remote-provider-error-text">{formRemoteError}</div>
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={loadFormRemoteProviders}
                    >
                      点击刷新
                    </button>
                  </div>
                ) : formRemoteItems.length === 0 ? (
                  <div className="empty-state-mini">
                    {config?.serverUrl
                      ? "点击「查询」从服务端获取服务商"
                      : "请先在设置页配置服务端地址"}
                  </div>
                ) : (
                  formRemoteItems.map((it) => {
                    const meta = getProviderTypeMeta(it.type);
                    const selected = formRemoteSelectedId === it.remoteId;
                    const exists = existingProviderNames.has(it.name);
                    return (
                      <div
                        key={it.remoteId}
                        className={`remote-provider-item remote-provider-pickable ${selected ? "selected" : ""}`}
                        onClick={() => pickFormRemote(it)}
                      >
                        <div className="remote-item-body">
                          <div className="remote-item-header">
                            <span className="remote-item-name">{it.name}</span>
                            <span className={`provider-type-badge ${meta.cssClass}`}>
                              {it.typeDesc || meta.label}
                            </span>
                            {exists && <span className="remote-item-exists">本地已存在</span>}
                          </div>
                          <code className="remote-item-url">
                            {it.apiUrl || "未配置 API 地址"}
                          </code>
                          {it.tagList && it.tagList.length > 0 && (
                            <div className="remote-item-tags">
                              {it.tagList.slice(0, 6).map((tag) => (
                                <span key={tag} className="tag-pill">
                                  {tag}
                                </span>
                              ))}
                              {it.tagList.length > 6 && (
                                <span className="tag-pill more">
                                  +{it.tagList.length - 6}
                                </span>
                              )}
                            </div>
                          )}
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function shortenUrl(url: string, max = 32): string {
  try {
    const u = new URL(url);
    return (u.host + u.pathname).slice(0, max);
  } catch {
    return url.slice(0, max);
  }
}
