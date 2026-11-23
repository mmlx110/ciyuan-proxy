import { useState, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";

export interface UpdateStatus {
  currentVersion: string;
  latestVersion: string | null;
  isChecking: boolean;
  isDownloading: boolean;
  isAvailable: boolean;
  error: string | null;
  message: string | null;
}

export function useUpdater() {
  const [status, setStatus] = useState<UpdateStatus>({
    currentVersion: "0.0.0",
    latestVersion: null,
    isChecking: false,
    isDownloading: false,
    isAvailable: false,
    error: null,
    message: null,
  });

  const endpointsRef = useRef<string[]>([]);

  /**
   * 最近一次检查到的安装包直链(manifest 根级 url)。
   * 用 ref 而不是局部变量,是因为 catch 块要拿它做错误提示 ——
   * try 块里的 const 在 catch 里已经出了作用域。
   */
  const downloadUrlRef = useRef<string>("");

  /**
   * 让 Rust 侧把替换掉占位符后的真实请求地址写进日志文件与「日志」页，
   * 并把地址缓存下来用于错误提示。
   *
   * <p>tauri-plugin-updater 的报错（"update endpoint did not respond with a successful
   * status code"）不带请求 URL，只给结论，排查时无从下手，所以这里主动先打一行。
   */
  const logEndpoints = useCallback(async () => {
    try {
      endpointsRef.current = await invoke<string[]>("log_updater_endpoints");
    } catch (e) {
      console.error("[updater] failed to log endpoints:", e);
    }
  }, []);

  /**
   * 把错误信息里的地址补全。
   *
   * <p>插件的报错分两类，失败的是<b>不同</b>的请求：
   * <ul>
   *   <li>检查更新失败 → 请求的是<b>清单接口</b>（endpoints），
   *       报错如 "update endpoint did not respond with a successful status code"</li>
   *   <li>下载/安装失败 → 请求的是 manifest 里的 <b>url</b>（安装包直链），
   *       报错如 "Download request failed with status: 404 Not Found"</li>
   * </ul>
   * 两类都套上清单地址会误导排查 —— 比如安装包 404 时提示的却是清单地址，
   * 根本看不出是哪个地址挂了。所以这里按调用场景传入真正失败的地址。
   *
   * @param e        插件抛出的原始错误
   * @param failedUrl 真正失败的请求地址；为空时回退到清单地址
   */
  const describeError = useCallback((e: unknown, failedUrl?: string) => {
    const message = String(e);
    const url = failedUrl ?? endpointsRef.current.join(" | ");
    if (!url) {
      return message;
    }
    return `${message}(请求地址: ${url})`;
  }, []);

  const showMessage = useCallback((message: string) => {
    setStatus((prev) => ({ ...prev, message }));
    setTimeout(() => {
      setStatus((prev) => ({ ...prev, message: null }));
    }, 3000);
  }, []);

  const showError = useCallback((error: string) => {
    setStatus((prev) => ({ ...prev, error }));
    setTimeout(() => {
      setStatus((prev) => ({ ...prev, error: null }));
    }, 3000);
  }, []);

  const loadCurrentVersion = useCallback(async () => {
    try {
      const version = await invoke<string>("get_current_version");
      setStatus((prev) => ({ ...prev, currentVersion: version }));
    } catch (e) {
      console.error("Failed to get current version:", e);
    }
  }, []);

  const checkForUpdates = useCallback(async () => {
    setStatus((prev) => ({ ...prev, isChecking: true, error: null }));

    try {
      await logEndpoints();
      const update = await check();

      if (update?.available) {
        const downloadUrl = String(update.rawJson.url ?? "");
        downloadUrlRef.current = downloadUrl;
        console.info(`[updater] 有新版本 v${update.version},下载地址 ${downloadUrl || "(无 url 字段)"}`);
        setStatus((prev) => ({
          ...prev,
          isChecking: false,
          isAvailable: true,
          latestVersion: update.version,
          message: `有新版本可用: v${update.version}`,
        }));
      } else {
        setStatus((prev) => ({
          ...prev,
          isChecking: false,
          isAvailable: false,
          latestVersion: null,
        }));
        showMessage("当前已是最新版本");
      }
    } catch (e) {
      const errorMsg = describeError(e);
      console.error("[updater] check failed:", e);
      setStatus((prev) => ({
        ...prev,
        isChecking: false,
        isAvailable: false,
      }));
      showError(errorMsg);
    }
  }, [describeError, logEndpoints, showError, showMessage]);

  const downloadAndInstallUpdate = useCallback(async () => {
    setStatus((prev) => ({ ...prev, isDownloading: true, error: null }));

    try {
      await logEndpoints();
      const update = await check();
      if (update) {
        // 安装包直链:Dynamic 格式下就是 manifest 根级的 url 字段。
        // 下载失败时用它做错误提示,否则会误报成清单地址。
        const downloadUrl = String(update.rawJson.url ?? "");
        downloadUrlRef.current = downloadUrl;
        console.info(`[updater] 开始下载 ${downloadUrl || "(无 url 字段)"}`);
        await update.downloadAndInstall();
        setStatus((prev) => ({
          ...prev,
          isDownloading: false,
        }));
        showMessage("更新安装成功，应用将重启");
      } else {
        setStatus((prev) => ({
          ...prev,
          isDownloading: false,
        }));
        showMessage("当前已是最新版本，无需更新");
      }
    } catch (e) {
      // 下载/安装阶段失败时,真正失败的是安装包直链而不是清单地址,
      // 这里带上直链才能定位到是文件缺失(404)还是网络问题。
      const errorMsg = describeError(e, downloadUrlRef.current || undefined);
      console.error("[updater] download/install failed:", e);
      setStatus((prev) => ({
        ...prev,
        isDownloading: false,
      }));
      showError(errorMsg);
    }
  }, [describeError, logEndpoints, showError, showMessage]);


  return {
    status,
    loadCurrentVersion,
    checkForUpdates,
    downloadAndInstallUpdate,
  };
}