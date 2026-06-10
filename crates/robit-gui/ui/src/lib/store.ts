import { create } from "zustand";
import type {
  SessionInfo,
  MessageData,
  ConfigInfo,
  ToolCallInfo,
} from "./types";

interface AppStore {
  // Session list
  sessions: SessionInfo[];
  activeSessionId: string | null;

  // Messages grouped by session
  messages: Record<string, MessageData[]>;

  // Streaming text buffer per session
  streamingBuffer: Record<string, string>;

  // Agent status per session
  agentStatus: Record<string, "idle" | "ready" | "running">;

  // Pending tool confirmations
  pendingConfirms: Record<string, ToolCallInfo>;

  // Config
  config: ConfigInfo | null;

  // Sidebar width
  sidebarWidth: number;

  // Actions
  setSessions: (sessions: SessionInfo[]) => void;
  setActiveSession: (id: string | null) => void;
  setMessages: (sessionId: string, messages: MessageData[]) => void;
  appendStreaming: (sessionId: string, delta: string) => void;
  commitStreaming: (sessionId: string) => void;
  clearStreaming: (sessionId: string) => void;
  setAgentStatus: (
    sessionId: string,
    status: "idle" | "ready" | "running"
  ) => void;
  addToolCard: (sessionId: string, info: ToolCallInfo) => void;
  updateToolCard: (
    sessionId: string,
    toolCallId: string,
    updates: Partial<ToolCallInfo>
  ) => void;
  removeToolCard: (sessionId: string, toolCallId: string) => void;
  setConfig: (config: ConfigInfo) => void;
  setSidebarWidth: (width: number) => void;
  addSession: (session: SessionInfo) => void;
  removeSession: (sessionId: string) => void;
  updateSessionTitle: (sessionId: string, title: string) => void;
}

export const useStore = create<AppStore>((set, get) => ({
  sessions: [],
  activeSessionId: null,
  messages: {},
  streamingBuffer: {},
  agentStatus: {},
  pendingConfirms: {},
  config: null,
  sidebarWidth: Number(localStorage.getItem("sidebarWidth") || 220),

  setSessions: (sessions) => set({ sessions }),

  setActiveSession: (id) => set({ activeSessionId: id }),

  setMessages: (sessionId, messages) =>
    set((state) => ({
      messages: { ...state.messages, [sessionId]: messages },
    })),

  appendStreaming: (sessionId, delta) =>
    set((state) => ({
      streamingBuffer: {
        ...state.streamingBuffer,
        [sessionId]: (state.streamingBuffer[sessionId] || "") + delta,
      },
    })),

  commitStreaming: (sessionId) => {
    const buffer = get().streamingBuffer[sessionId];
    if (!buffer) return;
    const msg: MessageData = {
      id: Date.now(),
      role: "assistant",
      content: buffer,
      created_at: new Date().toISOString(),
    };
    set((state) => ({
      messages: {
        ...state.messages,
        [sessionId]: [...(state.messages[sessionId] || []), msg],
      },
      streamingBuffer: { ...state.streamingBuffer, [sessionId]: "" },
    }));
  },

  clearStreaming: (sessionId) =>
    set((state) => ({
      streamingBuffer: { ...state.streamingBuffer, [sessionId]: "" },
    })),

  setAgentStatus: (sessionId, status) =>
    set((state) => ({
      agentStatus: { ...state.agentStatus, [sessionId]: status },
      sessions: state.sessions.map((s) =>
        s.id === sessionId ? { ...s, status } : s
      ),
    })),

  addToolCard: (_sessionId, info) => {
    set((state) => ({
      pendingConfirms: {
        ...state.pendingConfirms,
        [info.tool_call_id]: info,
      },
    }));
  },

  updateToolCard: (_sessionId, toolCallId, updates) => {
    set((state) => ({
      pendingConfirms: {
        ...state.pendingConfirms,
        [toolCallId]: {
          ...state.pendingConfirms[toolCallId],
          ...updates,
        },
      },
    }));
  },

  removeToolCard: (_sessionId, toolCallId) => {
    set((state) => {
      const next = { ...state.pendingConfirms };
      delete next[toolCallId];
      return { pendingConfirms: next };
    });
  },

  setConfig: (config) => set({ config }),

  setSidebarWidth: (width) => {
    localStorage.setItem("sidebarWidth", String(width));
    set({ sidebarWidth: width });
  },

  addSession: (session) =>
    set((state) => ({
      sessions: [session, ...state.sessions],
      agentStatus: { ...state.agentStatus, [session.id]: "ready" },
    })),

  removeSession: (sessionId) =>
    set((state) => ({
      sessions: state.sessions.filter((s) => s.id !== sessionId),
    })),

  updateSessionTitle: (sessionId, title) =>
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === sessionId ? { ...s, title } : s
      ),
    })),
}));
