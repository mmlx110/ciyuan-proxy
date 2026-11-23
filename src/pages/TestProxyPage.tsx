import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useProviderKeys } from "../hooks/useProviderKeys";
import {
  proxyKeyApi,
  providerApi,
  splitModels,
  ProxyKey,
  ProxyApiType,
  Provider,
  PROXY_API_TYPE_CHAT,
  PROXY_API_TYPE_ANTHROPIC,
  PROXY_API_TYPE_RESPONSES,
} from "../api";

interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  displayContent: string;
  timestamp: Date;
  status: "sending" | "sent" | "error";
  responseTime?: number;
}

interface ProviderKeyInfo {
  name: string;
  model: string;
  providerName: string;
  apiUrl: string;
  apiType: ProxyApiType;
}

/// Anthropic Messages API 必填的 max_tokens 默认值，与 Rust 端常量保持一致
const ANTHROPIC_DEFAULT_MAX_TOKENS = 4096;

/// 测试页目标路径由 apiType 自动决定：
///   - Anthropic → /v1/messages
///   - chat(OpenAI 兼容) → /v1/chat/completions
///   - responses → /v1/responses

export function TestProxyPage() {
  const { providerKeys, loadProviderKeys } = useProviderKeys();
  const [proxyKeys, setProxyKeys] = useState<ProxyKey[]>([]);
  const [providers, setProviders] = useState<Provider[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(() => {
    const saved = localStorage.getItem("selectedProxyToken");
    return saved || null;
  });
const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [inputMessage, setInputMessage] = useState("");
  const [isSending, setIsSending] = useState(false);
  const [proxyStatus, setProxyStatus] = useState(false);
  const [config, setConfig] = useState<{ listenPort: number } | null>(null);
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);
  const [providerKeyInfo, setProviderKeyInfo] = useState<ProviderKeyInfo | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  const loadProxyKeys = useCallback(async () => {
    try {
      const data = await proxyKeyApi.getAll();
      // 仅保留已激活且关联了服务商令牌的代理令牌
      const activeKeys = data.filter(k => k.isActive && k.providerKeyId !== null);
      setProxyKeys(activeKeys);
    } catch (e) {
      console.error("Failed to load proxy keys:", e);
    }
  }, []);

  const loadConfig = useCallback(async () => {
    try {
      const data = invoke<{ listenPort: number }>("get_config");
      setConfig(await data);
    } catch (e) {
      console.error("Failed to load config:", e);
    }
  }, []);

  const loadProviders = useCallback(async () => {
    try {
      const data = await providerApi.getAll();
      setProviders(data);
    } catch (e) {
      console.error("Failed to load providers:", e);
    }
  }, []);

  const checkProxyStatus = useCallback(async () => {
    try {
      const status = await invoke<boolean>("get_proxy_status");
      setProxyStatus(status);
    } catch (e) {
      console.error("Failed to check proxy status:", e);
    }
  }, []);

  // 根据选中的代理令牌推导转发配置：优先 proxyKey.model → 否则 provider.models 首个
  const updateProviderKeyInfo = useCallback(() => {
    if (!selectedKey) {
      setProviderKeyInfo(null);
      return;
    }

    const key = proxyKeys.find(k => k.id === selectedKey);
    if (!key || !key.providerKeyId) {
      setProviderKeyInfo(null);
      return;
    }

    const providerKey = providerKeys.find(pk => pk.id === key.providerKeyId);
    if (!providerKey) {
      setProviderKeyInfo(null);
      return;
    }

    const provider = providers.find(p => p.id === providerKey.providerId);
    const apiType = key.apiType ?? PROXY_API_TYPE_CHAT;
    // 与 Rust 端 select_remote_url 保持一致：
    //   anthropic 模式优先 provider.apiAnthropicUrl，未配置时回退到 apiUrl；
    //   chat 模式直接使用 apiUrl。
    const apiUrl = (() => {
      if (!provider) return "Unknown";
      if (apiType === PROXY_API_TYPE_ANTHROPIC) {
        const anthropicUrl = provider.apiAnthropicUrl?.trim();
        if (anthropicUrl) return anthropicUrl;
      }
      return provider.apiUrl?.trim() || "Unknown";
    })();
    const providerName = providerKey.providerName || provider?.name || "Unknown";
    // 优先使用代理令牌绑定的模型，否则取 provider.models 的首个模型
    const model =
      (key.model && key.model.trim()) ||
      (provider ? splitModels(provider.models)[0] : undefined) ||
      "Unknown";

    setProviderKeyInfo({
      name: providerKey.name,
      model,
      providerName,
      apiUrl,
      apiType,
    });
  }, [selectedKey, proxyKeys, providerKeys, providers]);

  useEffect(() => {
    loadProxyKeys();
    loadProviderKeys();
    loadConfig();
    loadProviders();
    checkProxyStatus();
  }, [loadProxyKeys, loadProviderKeys, loadConfig, loadProviders, checkProxyStatus]);

  useEffect(() => {
    updateProviderKeyInfo();
  }, [updateProviderKeyInfo]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const showMessage = (type: "success" | "error", text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  };

  const startProxy = async () => {
    try {
      await invoke("start_proxy_server");
      setTimeout(checkProxyStatus, 500);
      showMessage("success", "代理服务器已启动");
    } catch (e) {
      showMessage("error", `启动失败: ${e}`);
    }
  };

  const stopProxy = async () => {
    try {
      await invoke("stop_proxy_server");
      setTimeout(checkProxyStatus, 500);
      showMessage("success", "代理服务器已停止");
    } catch (e) {
      showMessage("error", `停止失败: ${e}`);
    }
  };

  const handleSendMessage = async () => {
    if (!inputMessage.trim()) return;
    if (!selectedKey) {
      showMessage("error", "请先选择一个令牌");
      return;
    }
    if (!proxyStatus) {
      showMessage("error", "请先启动代理服务器");
      return;
    }

    const key = proxyKeys.find(k => k.id === selectedKey);
    if (!key) {
      showMessage("error", "未找到选中的令牌");
      return;
    }

    const userMessage: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content: inputMessage,
      displayContent: inputMessage,
      timestamp: new Date(),
      status: "sending",
    };

    setMessages(prev => [...prev, userMessage]);
    setIsSending(true);
    setInputMessage("");

    try {
      const apiType = providerKeyInfo?.apiType ?? PROXY_API_TYPE_CHAT;
      const isAnthropic = apiType === PROXY_API_TYPE_ANTHROPIC;
      const isResponses = apiType === PROXY_API_TYPE_RESPONSES;
      // 目标路径由 apiType 自动决定：
      //   Anthropic → /v1/messages
      //   Responses  → /v1/responses
      //   chat(OpenAI 兼容) → /v1/chat/completions
      const targetPath = isAnthropic
        ? "/v1/messages"
        : isResponses
          ? "/v1/responses"
          : "/v1/chat/completions";
      const url = `http://localhost:${config?.listenPort || 20112}${targetPath}`;

      const headers: Record<string, string> = {
        "Authorization": `Bearer ${key.key}`,
        "Content-Type": "application/json",
        "Origin": "http://localhost:1420",
      };

      const modelName = providerKeyInfo?.model || "gpt-3.5-turbo";
      const conversation = messages.map(m => ({
        role: m.role,
        content: m.content,
      })).concat([{ role: "user", content: inputMessage }]);

      // 构造请求体：
      //   Anthropic → messages + 必填 max_tokens
      //   Responses  → input 数组（与 messages 等价），无 max_tokens
      //   chat(OpenAI 兼容) → messages
      const payload: Record<string, unknown> = isAnthropic
        ? {
            model: modelName,
            max_tokens: ANTHROPIC_DEFAULT_MAX_TOKENS,
            messages: conversation,
            stream: true,
          }
        : isResponses
          ? {
              model: modelName,
              input: conversation,
              stream: true,
            }
          : {
              model: modelName,
              messages: conversation,
              stream: true,
            };

      const requestBody = JSON.stringify(payload);

      console.log("Sending request to:", url);
      console.log("Headers:", headers);
      console.log("Body:", requestBody);

      const startTime = Date.now();
      const res = await fetch(url, {
        method: "POST",
        headers,
        body: requestBody,
        mode: 'cors',
        credentials: 'same-origin',
      });

      console.log("Response status:", res.status);
      console.log("Response headers:", res.headers);

      setMessages(prev => prev.map(m =>
        m.id === userMessage.id ? { ...m, status: "sent" as const } : m
      ));

      if (res.ok) {
        const reader = res.body?.getReader();
        if (!reader) {
          throw new Error("No response body");
        }

        const decoder = new TextDecoder('utf-8');
        let accumulatedContent = "";
        let buffer = "";
        // Anthropic SSE 每行形如 `event: message_start` / `data: {...}`；
        // 通过 currentEvent 在收到 `event: content_block_delta` 时才解析 data 中的文本增量。
        let currentEvent = "";
        const assistantMessageId = `msg-${Date.now()}-assistant`;

        const assistantMessage: ChatMessage = {
          id: assistantMessageId,
          role: "assistant",
          content: "",
          displayContent: "",
          timestamp: new Date(),
          status: "sent",
          responseTime: 0,
        };
        setMessages(prev => [...prev, assistantMessage]);

        const flushAccumulated = () => {
          if (!accumulatedContent) return;
          const stripped = accumulatedContent.replace(/<think>[\s\S]*?<\/think>/g, '').trim();
          setMessages(prev => prev.map(m => {
            if (m.id === assistantMessageId) {
              return {
                ...m,
                content: stripped,
                displayContent: stripped,
              };
            }
            return m;
          }));
        };

        while (true) {
          const { done, value } = await reader.read();

          if (done) {
            break;
          }

          buffer += decoder.decode(value, { stream: true });
          // 按双换行切分 SSE 事件
          let eventEnd;
          while ((eventEnd = buffer.indexOf("\n\n")) !== -1) {
            const rawEvent = buffer.slice(0, eventEnd);
            buffer = buffer.slice(eventEnd + 2);

            currentEvent = "";
            const dataLines: string[] = [];
            for (const line of rawEvent.split("\n")) {
              if (line.startsWith("event: ")) {
                currentEvent = line.slice(7).trim();
              } else if (line.startsWith("data: ")) {
                dataLines.push(line.slice(6));
              }
            }
            const dataStr = dataLines.join("\n");
            if (!dataStr) continue;

            try {
              const json = JSON.parse(dataStr);
              if (isAnthropic) {
                // Anthropic 流式：content_block_delta.delta.text 携带增量文本
                if (
                  currentEvent === "content_block_delta" &&
                  json?.delta?.type === "text_delta" &&
                  typeof json.delta.text === "string"
                ) {
                  accumulatedContent += json.delta.text;
                  flushAccumulated();
                }
              } else if (isResponses) {
                // OpenAI Responses API（Codex）流式：
                //   event: response.output_text.delta  → data.delta = "..."  文本增量
                // 也兼容无 event 行的实现：直接读 data.delta 或 data.text。
                if (dataStr === "[DONE]") continue;
                const deltaText =
                      (typeof json?.delta === "string" ? json.delta : null) ??
                      (typeof json?.text === "string" ? json.text : null);
                if (deltaText) {
                  accumulatedContent += deltaText;
                  flushAccumulated();
                }
              } else {
                // OpenAI 流式：choices[0].delta.content
                if (dataStr === "[DONE]") continue;
                const delta = json.choices?.[0]?.delta?.content;
                if (delta) {
                  accumulatedContent += delta;
                  flushAccumulated();
                }
              }
            } catch (e) {
              console.log("Parse error (possibly incomplete):", e);
            }
          }
        }
        
        const responseTime = Date.now() - startTime;
        setMessages(prev => prev.map(m => {
          if (m.id === assistantMessageId) {
            return { 
              ...m, 
              content: accumulatedContent.replace(/<think>[\s\S]*?<\/think>/g, '').trim(),
              displayContent: accumulatedContent.replace(/<think>[\s\S]*?<\/think>/g, '').trim(),
              responseTime,
            };
          }
          return m;
        }));
        
        console.log("Streaming complete, total time:", responseTime);
      } else {
        const responseTime = Date.now() - startTime;
        const errorText = await res.text();
        console.error("Error response:", errorText);
        const errorMessage: ChatMessage = {
          id: `msg-${Date.now()}`,
          role: "assistant",
          content: `Error: ${res.status} - ${errorText}`,
          displayContent: `Error: ${res.status} - ${errorText}`,
          timestamp: new Date(),
          status: "error",
          responseTime,
        };
        setMessages(prev => [...prev, errorMessage]);
      }
    } catch (error) {
      console.error("Send message error:", error);
      console.error("Error type:", typeof error);
      console.error("Error details:", JSON.stringify(error, Object.getOwnPropertyNames(error)));
      
      let errorMsg = "Unknown error";
      if (error instanceof TypeError) {
        errorMsg = `TypeError: ${error.message}`;
      } else if (error instanceof Error) {
        errorMsg = error.message;
      } else {
        errorMsg = String(error);
      }
      
      console.error("Final error message:", errorMsg);
      
      setMessages(prev => {
        const updated = prev.map(m => 
          m.id === userMessage.id ? { ...m, status: "error" as const } : m
        );
        const errorMessage: ChatMessage = {
          id: `msg-${Date.now()}`,
          role: "assistant",
          content: `Failed to send: ${errorMsg}`,
          displayContent: `Failed to send: ${errorMsg}`,
          timestamp: new Date(),
          status: "error",
        };
        return [...updated, errorMessage];
      });
    } finally {
      setIsSending(false);
    }
  };

  const handleKeyPress = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSendMessage();
    }
  };

  const clearChat = () => {
    setMessages([]);
  };

  return (
    <div className="test-proxy-page">
      {message && (
        <div className={`message ${message.type}`}>
          {message.text}
        </div>
      )}

      <div className="chat-container">
        <div className="chat-header">
          <div className="header-left">
            <h2>🔄 代理测试 - 对话模式</h2>
            <div className="connection-status">
              <span className={`status-indicator ${proxyStatus ? "active" : "inactive"}`}></span>
              <span>{proxyStatus ? "代理服务器运行中" : "代理服务器已停止"}</span>
            </div>
          </div>
          <div className="header-actions">
            {!proxyStatus ? (
              <button className="btn btn-primary" onClick={startProxy}>
                启动代理
              </button>
            ) : (
              <button className="btn btn-danger" onClick={stopProxy}>
                停止代理
              </button>
            )}
            <button className="btn btn-secondary" onClick={clearChat}>
              清空对话
            </button>
          </div>
        </div>

        <div className="chat-inner">
          <div className="chat-sidebar">
          <div className="section">
            <h3>选择代理 Key</h3>
            <select
              value={selectedKey || ""}
              onChange={(e) => {
                const value = e.target.value || null;
                setSelectedKey(value);
                if (value) {
                  localStorage.setItem("selectedProxyToken", value);
                } else {
                  localStorage.removeItem("selectedProxyToken");
                }
              }}
              className="token-select"
              disabled={!proxyStatus}
            >
              <option value="">请选择代理 Key</option>
              {proxyKeys.map(key => (
                <option key={key.id} value={key.id}>
                  {key.name} ({key.key.slice(0, 8)}...)
                </option>
              ))}
            </select>
          </div>

          {providerKeyInfo && (
            <div className="section">
              <h3>转发配置</h3>
              <div className="config-grid">
                <div className="config-item">
                  <span className="config-label">服务商</span>
                  <span className="config-value">{providerKeyInfo.providerName}</span>
                </div>
                <div className="config-item">
                  <span className="config-label">目标URL</span>
                  <span className="config-value url-value">{providerKeyInfo.apiUrl}</span>
                </div>
                <div className="config-item">
                  <span className="config-label">服务商令牌</span>
                  <span className="config-value">{providerKeyInfo.name}</span>
                </div>
                <div className="config-item">
                  <span className="config-label">目标模型</span>
                  <span className="config-value model-tag">{providerKeyInfo.model}</span>
                </div>
                <div className="config-item">
                  <span className="config-label">API 类型</span>
                  <span className="config-value">
                    <span className={`api-type-badge api-type-${providerKeyInfo.apiType === PROXY_API_TYPE_ANTHROPIC ? "anthropic" : providerKeyInfo.apiType === PROXY_API_TYPE_RESPONSES ? "responses" : "openai"}`}>
                      {providerKeyInfo.apiType === PROXY_API_TYPE_ANTHROPIC
                        ? "anthropic"
                        : providerKeyInfo.apiType === PROXY_API_TYPE_RESPONSES
                          ? "responses"
                          : "chat"}
                    </span>
                  </span>
                </div>
              </div>
            </div>
          )}

          <div className="section">
            <h3>转发流程</h3>
            <div className="flow-diagram">
              <div className="flow-step">
                <span className="step-number">1</span>
                <span className="step-text">请求到达代理</span>
              </div>
              <div className="flow-arrow">→</div>
              <div className="flow-step">
                <span className="step-number">2</span>
                <span className="step-text">Token匹配服务商令牌</span>
              </div>
              <div className="flow-arrow">→</div>
              <div className="flow-step">
                <span className="step-number">3</span>
                <span className="step-text">注入API Key</span>
              </div>
              <div className="flow-arrow">→</div>
              <div className="flow-step">
                <span className="step-number">4</span>
                <span className="step-text">转发至目标</span>
              </div>
            </div>
          </div>
        </div>

        <div className="chat-main">
          <div className="messages-container">
            {messages.length === 0 ? (
              <div className="empty-state">
                <div className="empty-icon">💬</div>
                <p>选择令牌后开始对话</p>
                <p className="empty-hint">发送消息会通过代理服务器转发到目标API</p>
              </div>
            ) : (
              messages.map(msg => (
                <div key={msg.id} className={`message-bubble ${msg.role} ${msg.status}`}>
                  <div className="message-header">
                    <span className="message-role">
                      {msg.role === "user" ? "👤 我" : "🤖 助手"}
                    </span>
                    <span className="message-time">
                      {msg.timestamp.toLocaleTimeString()}
                    </span>
                    {msg.responseTime && (
                      <span className="response-time">
                        ⏱️ {msg.responseTime}ms
                      </span>
                    )}
                    {msg.status === "sending" && (
                      <span className="sending-indicator">发送中...</span>
                    )}
                    {msg.status === "error" && (
                      <span className="error-indicator">❌</span>
                    )}
                  </div>
                  <div className="message-content">
                    {msg.displayContent}
                  </div>
                </div>
              ))
            )}
            <div ref={messagesEndRef} />
          </div>

          <div className="input-container">
            <textarea
              value={inputMessage}
              onChange={(e) => setInputMessage(e.target.value)}
              onKeyPress={handleKeyPress}
              placeholder="输入消息..."
              className="message-input"
              disabled={!selectedKey || !proxyStatus || isSending}
              rows={2}
            />
            <button
              className="send-btn"
              onClick={handleSendMessage}
              disabled={!inputMessage.trim() || !selectedKey || !proxyStatus || isSending}
            >
              {isSending ? "发送中..." : "发送"}
            </button>
          </div>
        </div>
        </div>
      </div>
    </div>
  );
}