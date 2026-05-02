import type { Frame } from "./api";

const KEY = "screenrecall.chat.v1";

export type StoredMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  citations?: Frame[];
  stats?: {
    ms: number;
    chars: number;
    est_tps: number;
  };
};

export type StoredThread = {
  id: string;
  title: string;
  archived: boolean;
  messages: StoredMessage[];
};

export type StoredChatState = {
  threads: StoredThread[];
  activeThreadId: string;
  sessionId: string;
};

export function loadChatState(): StoredChatState | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredChatState>;
    if (!Array.isArray(parsed.threads) || typeof parsed.activeThreadId !== "string") {
      return null;
    }
    return {
      threads: parsed.threads as StoredThread[],
      activeThreadId: parsed.activeThreadId,
      sessionId: typeof parsed.sessionId === "string" ? parsed.sessionId : crypto.randomUUID(),
    };
  } catch {
    return null;
  }
}

export function saveChatState(state: StoredChatState): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch (e) {
    console.warn("Failed to save chat history", e);
  }
}
