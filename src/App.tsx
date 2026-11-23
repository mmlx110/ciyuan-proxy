import { useState, useEffect } from "react";
import { ProxyKeyPage } from "./pages/ProxyKeyPage";
import { ProviderKeyPage } from "./pages/ProviderKeyPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ProviderManagementPage } from "./pages/ProviderManagementPage";
import { TestProxyPage } from "./pages/TestProxyPage";
import { FeedbackPage } from "./pages/FeedbackPage";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type Page =
  | "tokens"
  | "token-groups"
  | "settings"
  | "providers"
  | "test-proxy"
  | "feedback";

interface NavItem {
  id: Page;
  label: string;
  hash: string;
}

const navItems: NavItem[] = [
  { id: "tokens", label: "代理秘钥", hash: "#/" },
  { id: "token-groups", label: "服务商秘钥", hash: "#/token-groups" },
  { id: "providers", label: "服务商", hash: "#/providers" },
  { id: "test-proxy", label: "代理测试", hash: "#/test-proxy" },
  { id: "settings", label: "设置", hash: "#/settings" },
];

// 官网地址：在 Tauri 桌面端通过系统浏览器打开。
const OFFICIAL_SITE_URL = "https://www.ciyuanproxy.com/";

async function openExternal(url: string) {
  try {
    // Tauri 桌面端：调用 opener 插件在系统默认浏览器中打开
    await openUrl(url);
  } catch (err) {
    // 浏览器开发环境的兜底：使用新窗口打开
    console.warn("openUrl failed, fallback to window.open", err);
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

function App() {
  const [ready, setReady] = useState(false);
  const [currentPage, setCurrentPage] = useState<Page>("tokens");

  useEffect(() => {
    setReady(true);

    const handleHashChange = () => {
      console.log("Hash changed:", window.location.hash);
      if (window.location.hash === "#/token-groups") {
        setCurrentPage("token-groups");
      } else if (window.location.hash === "#/settings") {
        setCurrentPage("settings");
      } else if (window.location.hash === "#/providers") {
        setCurrentPage("providers");
      } else if (window.location.hash === "#/test-proxy") {
        setCurrentPage("test-proxy");
      } else if (window.location.hash === "#/feedback") {
        setCurrentPage("feedback");
      } else {
        setCurrentPage("tokens");
      }
    };

    window.addEventListener("hashchange", handleHashChange);
    handleHashChange();

    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  if (!ready) {
    return (
      <div className="app">
        <div className="loading">初始化中...</div>
      </div>
    );
  }

  console.log("Current page:", currentPage);

  return (
    <div className="app">
      <header className="header">
        <div className="header-left">
          {/*<h1>CiYuanProxy</h1>*/}
        </div>
        <nav className="header-right">
          {navItems.map((item) => (
            <a
              key={item.id}
              href={item.hash}
              className={`nav-btn ${currentPage === item.id ? "active" : ""}`}
            >
              {item.label}
            </a>
          ))}
          <span className="header-divider" aria-hidden="true" />
          <a
            href="#/feedback"
            className={`nav-btn ${currentPage === "feedback" ? "active" : ""}`}
            title="提交意见反馈"
          >
            意见反馈
          </a>
          <button
            type="button"
            className="nav-btn nav-btn-external"
            onClick={() => openExternal(OFFICIAL_SITE_URL)}
            title="前往词元代理官网"
          >
            官网
          </button>
        </nav>
      </header>
      <main className="content">
        {currentPage === "tokens" && <ProxyKeyPage />}
        {currentPage === "token-groups" && <ProviderKeyPage />}
        {currentPage === "settings" && <SettingsPage />}
        {currentPage === "providers" && <ProviderManagementPage />}
        {currentPage === "test-proxy" && <TestProxyPage />}
        {currentPage === "feedback" && <FeedbackPage />}
      </main>
    </div>
  );
}

export default App;