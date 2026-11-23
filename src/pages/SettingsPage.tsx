import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useConfig } from "../hooks/useConfig";
import { useUpdater } from "../hooks/useUpdater";
import { statsApi } from "../api";

const ENV_BADGE: Record<string, { label: string; bg: string; fg: string }> = {
  dev: { label: "开发版", bg: "#e0e7ff", fg: "#4338ca" },
  test: { label: "测试版", bg: "#fed7aa", fg: "#c2410c" },
  prod: { label: "正式版", bg: "#d1fae5", fg: "#047857" },
};

function envBadgeStyle(appEnv: string): { label: string; bg: string; fg: string } {
  return (
    ENV_BADGE[appEnv] ?? { label: appEnv || "未知", bg: "#e5e7eb", fg: "#374151" }
  );
}

export function SettingsPage() {
  const { config, loading, error, loadConfig, updateConfig } = useConfig();
  const { status, loadCurrentVersion, checkForUpdates, downloadAndInstallUpdate } = useUpdater();
  const [editingPort, setEditingPort] = useState<number | null>(null);
  const [portInput, setPortInput] = useState("");
  const [editingServerUrl, setEditingServerUrl] = useState(false);
  const [serverUrlInput, setServerUrlInput] = useState("");
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);

  // 统计相关本地状态
  const [statsEnabled, setStatsEnabled] = useState<boolean | null>(null);
  const [bufferedCount, setBufferedCount] = useState<number | null>(null);
  const [flushing, setFlushing] = useState(false);

  const showMessage = (type: "success" | "error", text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  };

  useEffect(() => {
    loadCurrentVersion();
  }, [loadCurrentVersion]);

  // 进入设置页时加载统计开关状态与缓冲条数
  useEffect(() => {
    const loadStats = async () => {
      try {
        const [enabled, count] = await Promise.all([
          statsApi.getEnabled(),
          statsApi.getBufferedCount(),
        ]);
        setStatsEnabled(enabled);
        setBufferedCount(count);
      } catch (err) {
        // 后端还未实现统计模块时静默忽略，不影响其他功能
        console.warn("load stats state failed:", err);
      }
    };
    loadStats();
  }, []);

  const handleToggleStats = async () => {
    if (statsEnabled === null) return;
    const next = !statsEnabled;
    try {
      await statsApi.setEnabled(next);
      setStatsEnabled(next);
      showMessage("success", next ? "已开启匿名统计" : "已关闭匿名统计");
    } catch (err) {
      showMessage("error", `操作失败: ${err}`);
    }
  };

  const handleFlushStats = async () => {
    setFlushing(true);
    try {
      const report = await statsApi.flushNow();
      const count = await statsApi.getBufferedCount();
      setBufferedCount(count);
      if (report.serverReachable) {
        if (report.uploaded > 0) {
          showMessage("success", `已上报 ${report.uploaded} 条事件`);
        } else {
          showMessage("success", "服务端可达，无新事件需上报");
        }
      } else {
        showMessage("error", "无法连接服务端，已保留在本地缓冲");
      }
    } catch (err) {
      showMessage("error", `上报失败: ${err}`);
    } finally {
      setFlushing(false);
    }
  };

  const handleCopyUrl = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      showMessage("success", "地址已复制到剪贴板");
    } catch (err) {
      showMessage("error", "复制失败");
    }
  };

  const handleEditPort = () => {
    if (config) {
      setEditingPort(config.listenPort);
      setPortInput(config.listenPort.toString());
    }
  };

  const handleCancelPort = () => {
    setEditingPort(null);
    setPortInput("");
  };

  const handleSavePort = async () => {
    const port = parseInt(portInput, 10);
    if (isNaN(port) || port < 1 || port > 65535) {
      showMessage("error", "端口号必须在 1-65535 之间");
      return;
    }

    try {
      if (config) {
        const currentPort = config.listenPort;

        await updateConfig({
          listen_port: port,
          auto_start: config.autoStart,
          minimize_to_tray: config.minimizeToTray,
          log_level: config.logLevel,
          server_url: config.serverUrl,
        });

        if (currentPort !== port) {
          const isRunning = await invoke<boolean>("get_proxy_status");

          if (isRunning) {
            showMessage("success", "端口已更新，正在重启代理服务...");
            await invoke("stop_proxy_server");
            await new Promise(resolve => setTimeout(resolve, 500));
            await invoke("start_proxy_server");
            await new Promise(resolve => setTimeout(resolve, 500));
            showMessage("success", "端口已更新，代理服务已重启");
          } else {
            showMessage("success", "端口已更新，下次启动代理服务时生效");
          }
        } else {
          showMessage("success", "端口未变化");
        }

        setEditingPort(null);
      }
    } catch (err) {
      showMessage("error", `保存失败: ${err}`);
    }
  };

  const handleToggleAutoStart = async () => {
    try {
      if (config) {
        await updateConfig({
          listen_port: config.listenPort,
          auto_start: !config.autoStart,
          minimize_to_tray: config.minimizeToTray,
          log_level: config.logLevel,
          server_url: config.serverUrl,
        });
        showMessage("success", config.autoStart ? "已关闭开机自启" : "已开启开机自启");
      }
    } catch (err) {
      showMessage("error", `操作失败: ${err}`);
    }
  };

  const handleToggleMinimizeToTray = async () => {
    try {
      if (config) {
        await updateConfig({
          listen_port: config.listenPort,
          auto_start: config.autoStart,
          minimize_to_tray: !config.minimizeToTray,
          log_level: config.logLevel,
          server_url: config.serverUrl,
        });
        showMessage("success", config.minimizeToTray ? "已关闭最小化到托盘" : "已开启最小化到托盘");
      }
    } catch (err) {
      showMessage("error", `操作失败: ${err}`);
    }
  };

  const handleEditServerUrl = () => {
    if (config) {
      setEditingServerUrl(true);
      setServerUrlInput(config.serverUrl);
    }
  };

  const handleCancelServerUrl = () => {
    setEditingServerUrl(false);
    setServerUrlInput("");
  };

  const handleSaveServerUrl = async () => {
    const trimmed = serverUrlInput.trim();
    if (!trimmed) {
      showMessage("error", "远程服务端地址不能为空");
      return;
    }
    try {
      if (config) {
        await updateConfig({
          listen_port: config.listenPort,
          auto_start: config.autoStart,
          minimize_to_tray: config.minimizeToTray,
          log_level: config.logLevel,
          server_url: trimmed,
        });
        showMessage("success", "远程服务端地址已保存");
        setEditingServerUrl(false);
      }
    } catch (err) {
      showMessage("error", `保存失败: ${err}`);
    }
  };

  if (loading) {
    return (
      <div className="settings-page">
        <div className="loading">加载中...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="settings-page">
        <div className="error-state">
          <span>加载失败: {error}</span>
          <button onClick={loadConfig}>重试</button>
        </div>
      </div>
    );
  }

  if (!config) {
    return (
      <div className="settings-page">
        <div className="error-state">
          <span>无法获取配置</span>
          <button onClick={loadConfig}>重试</button>
        </div>
      </div>
    );
  }

  const serverUrl = `http://localhost:${config.listenPort}`;
  // 非正式版（dev / test）才允许调试远程服务端地址与匿名统计开关
  const showDebugSettings = config.appEnv !== "prod";

  return (
    <div className="settings-page">
      <div className="page-header">
        <div className="page-title">
          <h1>设置</h1>
          <span className="subtitle">配置本地代理服务参数</span>
        </div>
        {(() => {
          const badge = envBadgeStyle(config.appEnv);
          return (
            <span
              title={`构建环境：${config.appEnv}`}
              style={{
                padding: "4px 10px",
                borderRadius: 12,
                fontSize: 12,
                fontWeight: 600,
                background: badge.bg,
                color: badge.fg,
                whiteSpace: "nowrap",
              }}
            >
              {badge.label}
            </span>
          );
        })()}
      </div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <div className="settings-section">
        <h2>服务地址</h2>
        <div className="setting-item">
          <div className="setting-label">本地代理地址</div>
          <div className="setting-value">
            <code>{serverUrl}</code>
            <button
              className="btn-secondary copy-btn"
              onClick={() => handleCopyUrl(serverUrl)}
            >
              复制
            </button>
            <span className="setting-hint">这是你的本地代理服务地址</span>
          </div>
        </div>

        {showDebugSettings && (
          <div className="setting-item">
            <div className="setting-label">远程服务端地址</div>
            <div className="setting-value">
              {editingServerUrl ? (
                <div className="server-url-edit">
                  <input
                    type="text"
                    value={serverUrlInput}
                    onChange={(e) => setServerUrlInput(e.target.value)}
                    placeholder="https://api.ciyuanproxy.com/"
                    className="server-url-input"
                  />
                  <button className="btn-primary" onClick={handleSaveServerUrl}>
                    保存
                  </button>
                  <button className="btn-secondary" onClick={handleCancelServerUrl}>
                    取消
                  </button>
                </div>
              ) : (
                <div className="server-url-display">
                  <code>{config.serverUrl}</code>
                  <button className="btn-secondary" onClick={handleEditServerUrl}>
                    修改
                  </button>
                </div>
              )}
              <span className="setting-hint">从服务商管理页拉取远程服务商列表时所使用的基础地址，例如 https://api.ciyuanproxy.com/</span>
            </div>
          </div>
        )}
      </div>

      <div className="settings-section">
        <h2>代理端口</h2>
        <div className="setting-item">
          <div className="setting-label">监听端口</div>
          <div className="setting-value">
            {editingPort !== null ? (
              <div className="port-edit">
                <input
                  type="number"
                  value={portInput}
                  onChange={(e) => setPortInput(e.target.value)}
                  min="1"
                  max="65535"
                  className="port-input"
                />
                <button className="btn-primary" onClick={handleSavePort}>
                  保存
                </button>
                <button className="btn-secondary" onClick={handleCancelPort}>
                  取消
                </button>
              </div>
            ) : (
              <div className="port-display">
                <code>{config.listenPort}</code>
                <button className="btn-secondary" onClick={handleEditPort}>
                  修改
                </button>
              </div>
            )}
            <span className="setting-hint">修改端口后需要重启代理服务才能生效</span>
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h2>启动选项</h2>
        <div className="setting-item">
          <div className="setting-label">开机自启</div>
          <div className="setting-value">
            <button
              className={`toggle-btn ${config.autoStart ? "active" : "inactive"}`}
              onClick={handleToggleAutoStart}
            >
              {config.autoStart ? "开启" : "关闭"}
            </button>
            <span className="setting-hint">应用启动时自动开启代理服务</span>
          </div>
        </div>

        <div className="setting-item">
          <div className="setting-label">最小化到托盘</div>
          <div className="setting-value">
            <button
              className={`toggle-btn ${config.minimizeToTray ? "active" : "inactive"}`}
              onClick={handleToggleMinimizeToTray}
            >
              {config.minimizeToTray ? "开启" : "关闭"}
            </button>
            <span className="setting-hint">关闭窗口时最小化到系统托盘</span>
          </div>
        </div>
      </div>

      {showDebugSettings && (
        <div className="settings-section">
          <h2>匿名统计</h2>
          <div className="setting-item">
            <div className="setting-label">发送匿名使用统计</div>
            <div className="setting-value">
              <button
                className={`toggle-btn ${statsEnabled === false ? "inactive" : "active"}`}
                onClick={handleToggleStats}
                disabled={statsEnabled === null}
              >
                {statsEnabled === false ? "关闭" : "开启"}
              </button>
              <span className="setting-hint">
                仅上报启动/安装次数与版本、操作系统等聚合信息；不包含任何密钥、模型 URL 或请求内容
              </span>
            </div>
          </div>

          <div className="setting-item">
            <div className="setting-label">本地待上报事件</div>
            <div className="setting-value">
              <code>{bufferedCount === null ? "—" : `${bufferedCount} 条`}</code>
              <button
                className="btn-secondary"
                onClick={handleFlushStats}
                disabled={flushing || bufferedCount === 0}
              >
                {flushing ? "上报中..." : "立即上报"}
              </button>
              <span className="setting-hint">
                每 5 分钟自动批量上报一次；关闭上报开关后新事件仍会暂存在本地，重新开启时会一并补传
              </span>
            </div>
          </div>
        </div>
      )}

      <div className="settings-section">
        <h2>软件更新</h2>
        <div className="setting-item">
          <div className="setting-label">当前版本</div>
          <div className="setting-value">
            <code>v{status.currentVersion}</code>
            {status.latestVersion && (
              <span className="new-version">
                新版本: v{status.latestVersion}
              </span>
            )}
          </div>
        </div>

        <div className="setting-item">
          <div className="setting-label">检查更新</div>
          <div className="setting-value">
            {status.isChecking ? (
              <span className="loading-text">检查中...</span>
            ) : status.isDownloading ? (
              <span className="loading-text">下载安装中...</span>
            ) : status.isAvailable ? (
              <button className="btn-primary update-btn" onClick={downloadAndInstallUpdate}>
                立即更新
              </button>
            ) : (
              <button className="btn-secondary" onClick={checkForUpdates}>
                检查更新
              </button>
            )}
            <span className="setting-hint">定期检查更新以获取最新功能和安全修复</span>
          </div>
        </div>

        {status.message && (
          <div className={`message ${status.isAvailable ? "success" : "info"}`}>
            {status.message}
          </div>
        )}

        {status.error && (
          <div className="message error">
            {status.error}
          </div>
        )}
      </div>
    </div>
  );
}