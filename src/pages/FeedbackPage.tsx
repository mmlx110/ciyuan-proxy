import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  FEEDBACK_TYPE_OPTIONS,
  FeedbackSubmitInput,
  FeedbackType,
  feedbackApi,
} from "../api";
import { useConfig } from "../hooks/useConfig";

const TITLE_MAX_LEN = 100;
const CONTACT_MAX_LEN = 100;
const CONTENT_MAX_LEN = 2000;

export function FeedbackPage() {
  const { config, loadConfig } = useConfig();
  const [type, setType] = useState<FeedbackType>(2);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [contact, setContact] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<
    { type: "success" | "error" | "info"; text: string } | null
  >(null);
  const [version, setVersion] = useState<string>("0.0.0");
  const [systemInfo, setSystemInfo] = useState<string>("Unknown");

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      let appVersion = "Unknown";
      let tauriVersion = "Unknown";
      let osVersion = "Unknown";
      try {
        appVersion = await invoke<string>("get_current_version");
      } catch {
        // 静默失败，使用默认占位
      }
      try {
        tauriVersion = await invoke<string>("get_tauri_version");
      } catch {
        // 静默失败，使用默认占位
      }
      try {
        osVersion = await invoke<string>("get_os_version");
      } catch {
        // 静默失败，使用默认占位
      }
      if (cancelled) return;
      setVersion(appVersion);
      const arch = (navigator as unknown as { platform?: string }).platform || "Unknown";
      setSystemInfo(`${osVersion} / ${arch} / Tauri ${tauriVersion}`);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const titleLen = title.trim().length;
  const contentLen = content.trim().length;
  const contactLen = contact.trim().length;

  const titleInvalid = titleLen === 0 || titleLen > TITLE_MAX_LEN;
  const contentInvalid = contentLen === 0 || contentLen > CONTENT_MAX_LEN;
  const contactInvalid = contactLen > CONTACT_MAX_LEN;

  const canSubmit = useMemo(
    () =>
      !submitting &&
      !titleInvalid &&
      !contentInvalid &&
      !contactInvalid &&
      !!config?.serverUrl,
    [submitting, titleInvalid, contentInvalid, contactInvalid, config],
  );

  const showMessage = (
    type: "success" | "error" | "info",
    text: string,
  ) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 4000);
  };

  const handleReset = () => {
    setType(2);
    setTitle("");
    setContent("");
    setContact("");
  };

  const handleSubmit = async () => {
    if (!config?.serverUrl) {
      showMessage("error", "请先在设置中配置远程服务端地址");
      return;
    }
    if (titleInvalid || contentInvalid || contactInvalid) {
      showMessage("error", "请检查表单字段");
      return;
    }
    setSubmitting(true);
    try {
      const input: FeedbackSubmitInput = {
        userId: null,
        type,
        title: title.trim(),
        content: content.trim(),
        contact: contact.trim() || undefined,
        clientVersion: version,
        systemInfo,
      };
      await feedbackApi.submit(config.serverUrl, input);
      showMessage("success", "提交成功");
      handleReset();
    } catch (err) {
      showMessage("error", `提交失败: ${err}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="feedback-page">
      <div className="page-header">
        <h1>意见反馈</h1>
        <span className="subtitle">提交 Bug、建议或商务合作咨询</span>
      </div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <div className="feedback-section">
        <div className="feedback-field">
          <label className="feedback-label">反馈类型</label>
          <div className="feedback-type-options">
            {FEEDBACK_TYPE_OPTIONS.map((opt) => (
              <button
                key={opt.value}
                type="button"
                className={`type-option ${type === opt.value ? "active" : ""}`}
                onClick={() => setType(opt.value)}
              >
                <span className="type-option-label">{opt.label}</span>
                <span className="type-option-desc">{opt.description}</span>
              </button>
            ))}
          </div>
        </div>

        <div className="feedback-field">
          <label className="feedback-label">
            标题
            <span className="feedback-counter">
              {titleLen}/{TITLE_MAX_LEN}
            </span>
          </label>
          <input
            type="text"
            className="feedback-input"
            placeholder="一句话概括问题或建议"
            value={title}
            maxLength={TITLE_MAX_LEN + 50}
            onChange={(e) => setTitle(e.target.value)}
          />
        </div>

        <div className="feedback-field">
          <label className="feedback-label">
            内容
            <span className="feedback-counter">
              {contentLen}/{CONTENT_MAX_LEN}
            </span>
          </label>
          <textarea
            className="feedback-textarea"
            placeholder="详细描述问题复现步骤、期望结果、实际结果，或具体的优化建议"
            value={content}
            rows={8}
            maxLength={CONTENT_MAX_LEN + 200}
            onChange={(e) => setContent(e.target.value)}
          />
        </div>

        <div className="feedback-field">
          <label className="feedback-label">
            联系方式（选填）
            <span className="feedback-counter">
              {contactLen}/{CONTACT_MAX_LEN}
            </span>
          </label>
          <input
            type="text"
            className="feedback-input"
            placeholder="邮箱 / 手机号 / 微信，便于我们跟进处理"
            value={contact}
            maxLength={CONTACT_MAX_LEN + 50}
            onChange={(e) => setContact(e.target.value)}
          />
          <span className="feedback-hint">
            提交后我们会将处理结果通过该联系方式通知您；未填写则无法主动跟进。
          </span>
        </div>

        <div className="feedback-meta">
          <div className="meta-item">
            <span className="meta-label">客户端版本</span>
            <code>v{version}</code>
          </div>
          <div className="meta-item">
            <span className="meta-label">系统信息</span>
            <code>{systemInfo}</code>
          </div>
        </div>

        <div className="feedback-actions">
          <button
            className="btn-secondary"
            type="button"
            onClick={handleReset}
            disabled={submitting}
          >
            清空
          </button>
          <button
            className="btn-primary"
            type="button"
            onClick={handleSubmit}
            disabled={!canSubmit}
          >
            {submitting ? "提交中..." : "提交反馈"}
          </button>
        </div>
      </div>
    </div>
  );
}