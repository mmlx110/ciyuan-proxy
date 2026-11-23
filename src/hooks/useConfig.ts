import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface AppConfig {
  id: number;
  listenPort: number;
  autoStart: boolean;
  minimizeToTray: boolean;
  logLevel: string;
  serverUrl: string;
  /** 构建环境标识：dev / test / prod，由 .env.* 在构建时注入。 */
  appEnv: string;
}

export interface UpdateConfigDto {
  listen_port: number;
  auto_start: boolean;
  minimize_to_tray: boolean;
  log_level: string;
  server_url: string;
}

export function useConfig() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const cfg = await invoke<AppConfig>("get_config");
      setConfig(cfg);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  const updateConfig = async (data: UpdateConfigDto): Promise<void> => {
    await invoke<void>("update_config", {
      listenPort: data.listen_port,
      autoStart: data.auto_start,
      minimizeToTray: data.minimize_to_tray,
      logLevel: data.log_level,
      serverUrl: data.server_url,
    });
    await loadConfig();
  };

  return {
    config,
    loading,
    error,
    loadConfig,
    updateConfig,
  };
}