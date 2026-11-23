import { useState, useEffect, useRef, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

export function LogPage() {
  const [logs, setLogs] = useState<string[]>([]);
  const [proxyStatus, setProxyStatus] = useState(false);
  const logContainerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const unlisten = listen("proxy_log", (event) => {
      const message = event.payload as string;
      setLogs(prev => [...prev.slice(-500), message]);
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, []);

  useEffect(() => {
    if (logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs]);

  const checkProxyStatus = useCallback(async () => {
    try {
      const status = await invoke<boolean>("get_proxy_status");
      setProxyStatus(status);
    } catch (e) {
      console.error("Failed to check proxy status:", e);
    }
  }, []);

  useEffect(() => {
    checkProxyStatus();
    const interval = setInterval(checkProxyStatus, 2000);
    return () => clearInterval(interval);
  }, [checkProxyStatus]);

  const clearLogs = () => {
    setLogs([]);
  };

  const downloadLogs = () => {
    const content = logs.join("\n");
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `proxy-logs-${new Date().toISOString().slice(0, 19).replace(/:/g, "-")}.txt`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  return (
    <div className="log-page">
      <div className="page-header">
        <div className="page-title">
          <h1>📜 代理服务器日志</h1>
          <span className="subtitle">实时查看代理服务器运行日志</span>
        </div>
        <div className="page-actions">
          <span className={`status-badge ${proxyStatus ? "active" : "inactive"}`}>
            {proxyStatus ? "🟢 运行中" : "🔴 已停止"}
          </span>
          <button className="btn-secondary" onClick={clearLogs}>
            🗑️ 清空日志
          </button>
          <button className="btn-primary" onClick={downloadLogs}>
            📥 下载日志
          </button>
        </div>
      </div>

      <div className="log-container" ref={logContainerRef}>
        {logs.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">📋</div>
            <div className="empty-text">暂无日志</div>
            <div className="empty-hint">启动代理服务器后，日志将显示在这里</div>
          </div>
        ) : (
          <pre className="log-content">
            {logs.join("\n")}
          </pre>
        )}
      </div>

      <div className="log-footer">
        <span className="log-count">共 {logs.length} 条日志</span>
        <span className="log-hint">日志最多保留最近 500 条</span>
      </div>
    </div>
  );
}