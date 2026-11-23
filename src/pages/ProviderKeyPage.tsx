import { useState, useEffect, useMemo } from "react";
import { useProviderKeys, ProviderKeyWithStatus } from "../hooks/useProviderKeys";
import { ProviderKeyForm, ProviderKeyFormData } from "../components/ProviderKeyForm";
import { AUTH_MODE_BROWSER } from "../api";

export function ProviderKeyPage() {

  const {
    providerKeys,
    loading,
    error,
    loadProviderKeys,
    deleteProviderKey,
    toggleActive,
  } = useProviderKeys();

  const [showModal, setShowModal] = useState(false);
  const [editingKey, setEditingKey] = useState<ProviderKeyWithStatus | null>(null);
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);
  const [searchQuery, setSearchQuery] = useState("");

  useEffect(() => {
    loadProviderKeys();
  }, [loadProviderKeys]);

  const filteredKeys = useMemo(() => {
    return providerKeys.filter((k) => {
      if (!searchQuery.trim()) return true;
      const query = searchQuery.toLowerCase();
      return (
        k.name.toLowerCase().includes(query) ||
        (k.providerName?.toLowerCase().includes(query) ?? false)
      );
    });
  }, [providerKeys, searchQuery]);

  const showMessage = (type: "success" | "error", text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  };

  const handleAdd = () => {
    setEditingKey(null);
    setShowModal(true);
  };

  const handleEdit = (key: ProviderKeyWithStatus) => {
    setEditingKey(key);
    setShowModal(true);
  };

  const handleCloseModal = () => {
    setShowModal(false);
    setEditingKey(null);
  };

  const handleSuccess = () => {
    showMessage("success", editingKey ? "更新成功" : "添加成功");
    handleCloseModal();
    loadProviderKeys();
  };

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const handleDelete = (id: string) => {
    setConfirmDeleteId(id);
  };

  const confirmDelete = async () => {
    if (!confirmDeleteId) return;
    try {
      await deleteProviderKey(confirmDeleteId);
      showMessage("success", "删除成功");
      loadProviderKeys();
    } catch (err) {
      showMessage("error", `删除失败: ${err}`);
    } finally {
      setConfirmDeleteId(null);
    }
  };

  const cancelDelete = () => {
    setConfirmDeleteId(null);
  };

  const handleToggleActive = async (key: ProviderKeyWithStatus) => {
    try {
      await toggleActive(key.id, !key.isActive);
      showMessage("success", key.isActive ? "已停用" : "已激活");
      loadProviderKeys();
    } catch (err) {
      showMessage("error", `操作失败: ${err}`);
    }
  };

  return (
    <div className="token-group-page">
      <div className="page-header-wrapper">
        <div className="page-header">
          <div className="page-title">
            <h1>服务商令牌管理</h1>
            <span className="subtitle">管理远程代理令牌和路由配置</span>
          </div>
          <div className="page-actions">
            <button className="btn-primary" onClick={handleAdd}>
              + 添加服务商秘钥
            </button>
          </div>
        </div>

        <div className="search-bar">
          <div className="search-icon">🔍</div>
          <input
            type="text"
            placeholder="搜索miy名称、服务商..."
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
          共 <span className="results-count">{filteredKeys.length}</span> 个服务商令牌
          {searchQuery && (
            <span className="search-hint">（搜索 "{searchQuery}" 的结果）</span>
          )}
        </div>
      </div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <div className="token-group-list-container">
        {loading ? (
          <div className="loading">
            <div className="loading-spinner"></div>
            <span>加载中...</span>
          </div>
        ) : error ? (
          <div className="error-state">
            <div className="error-icon">❌</div>
            <span>加载失败: {error}</span>
            <button onClick={loadProviderKeys}>重试</button>
          </div>
        ) : filteredKeys.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">📭</div>
            <span>
              {searchQuery ? "未找到匹配的服务商令牌" : "暂无服务商令牌"}
            </span>
            {!searchQuery && <button onClick={handleAdd}>添加第一个服务商令牌</button>}
          </div>
        ) : (
          <div className="token-group-list">
            <div className="list-header">
              <span className="col-name">名称</span>
              <span className="col-provider">服务商</span>
              <span className="col-status">状态</span>
              <span className="col-actions">操作</span>
            </div>
            {filteredKeys.map((key) => (
              <div key={key.id} className="list-row">
                <div className="col-name">
                  <span className="name-text">{key.name}</span>
                </div>
                <div className="col-provider">
                  {key.providerName && (
                    <span className="provider-badge">{key.providerName}</span>
                  )}
                  {key.authType === AUTH_MODE_BROWSER && (
                    <span className="provider-badge auth-badge">浏览器授权</span>
                  )}
                </div>
                <div className="col-status">
                  <label className="toggle-switch">
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
                </div>
                <div className="col-actions">
                  <button className="action-btn edit" onClick={() => handleEdit(key)}>
                    ✏️ 编辑
                  </button>
                  <button
                    className="action-btn delete"
                    onClick={() => handleDelete(key.id)}
                  >
                    🗑️ 删除
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {showModal && (
<div className="modal-overlay">
            <div className="modal-content">
            <div className="modal-header">
              <h3>{editingKey ? "编辑服务商令牌" : "添加服务商令牌"}</h3>
              <button className="modal-close" onClick={handleCloseModal}>
                ×
              </button>
            </div>
            <div className="modal-form">
              <ProviderKeyForm
                initialData={editingKey as ProviderKeyFormData | null}
                onSuccess={handleSuccess}
                onCancel={handleCloseModal}
              />
            </div>
          </div>
        </div>
      )}

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
              <p>确定要删除此服务商令牌吗？此操作无法撤销。</p>
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
    </div>
  );
}
