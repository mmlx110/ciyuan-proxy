import { useState, useEffect, useCallback } from "react";
import { providerKeyApi, ProviderKeyWithStatus } from "../api";

export type { ProviderKeyWithStatus } from "../api";

export interface CreateProviderKeyDto {
  name: string;
  apiKey: string;
  providerId: string;
}

export interface UpdateProviderKeyDto {
  name: string;
  apiKey: string | null;
  providerId: string;
}

export function useProviderKeys() {
  const [providerKeys, setProviderKeys] = useState<ProviderKeyWithStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadProviderKeys = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const keys = await providerKeyApi.getAll();
      setProviderKeys(keys);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadProviderKeys();
  }, [loadProviderKeys]);

  const createProviderKey = async (data: CreateProviderKeyDto): Promise<ProviderKeyWithStatus> => {
    const key = await providerKeyApi.create(data.name, data.apiKey, data.providerId);
    await loadProviderKeys();
    return key;
  };

  const updateProviderKey = async (id: string, data: UpdateProviderKeyDto): Promise<void> => {
    await providerKeyApi.update(id, data.name, data.apiKey, data.providerId);
    await loadProviderKeys();
  };

  const deleteProviderKey = async (id: string): Promise<void> => {
    await providerKeyApi.delete(id);
  };

  const toggleActive = async (id: string, isActive: boolean): Promise<void> => {
    await providerKeyApi.toggleActive(id, isActive);
    await loadProviderKeys();
  };

  return {
    providerKeys,
    loading,
    error,
    loadProviderKeys,
    createProviderKey,
    updateProviderKey,
    deleteProviderKey,
    toggleActive,
  };
}
