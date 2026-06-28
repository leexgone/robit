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

  // Tool calls grouped by session, in order
  toolCalls: Record<string, string[]>;

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

// Create store with simplified implementation for zustand v5
export const useStore = create<AppStore>((set, get) => ({
  sessions: [],
  activeSessionId: null,
  messages: {},
  streamingBuffer: {},
  agentStatus: {},
  toolCalls: {},
  pendingConfirms: {},
  config: null,
  sidebarWidth: Number(localStorage.getItem("sidebarWidth") || 220),

  setSessions: (sessions) => set({ sessions }),

  setActiveSession: (id) => set({ activeSessionId: id }),

  setMessages: (sessionId, messages) =>
    set((state) => {
      // Check if messages actually changed to avoid unnecessary updates
      const current = state.messages[sessionId];
      if (current === messages) return {};
      if (current && current.length === messages.length) {
        let changed = false;
        for (let i = 0; i < current.length; i++) {
          if (current[i].id !== messages[i].id || current[i].content !== messages[i].content) {
            changed = true;
            break;
          }
        }
        if (!changed) return {};
      }
      return {
        messages: { ...state.messages, [sessionId]: messages },
      };
    }),

  appendStreaming: (sessionId, delta) =>
    set((state) => {
      const current = state.streamingBuffer[sessionId] || "";
      return {
        streamingBuffer: {
          ...state.streamingBuffer,
          [sessionId]: current + delta,
        },
      };
    }),

  commitStreaming: (sessionId) => {
    const state = get();
    const buffer = state.streamingBuffer[sessionId];
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
    set((state) => {
      if (!state.streamingBuffer[sessionId]) return {};
      return {
        streamingBuffer: { ...state.streamingBuffer, [sessionId]: "" },
      };
    }),

  setAgentStatus: (sessionId, status) =>
    set((state) => {
      const currentStatus = state.agentStatus[sessionId];
      if (currentStatus === status) return {};

      return {
        agentStatus: { ...state.agentStatus, [sessionId]: status },
        sessions: state.sessions.map((s) =>
          s.id === sessionId ? { ...s, status } : s
        ),
      };
    }),

  addToolCard: (sessionId, info) => {
    set((state) => {
      const currentToolCalls = state.toolCalls[sessionId] || [];
      const hasToolCall = currentToolCalls.includes(info.tool_call_id);

      if (hasToolCall && state.pendingConfirms[info.tool_call_id]) {
        // Already exists and pending, no change needed
        return {};
      }

      return {
        pendingConfirms: {
          ...state.pendingConfirms,
          [info.tool_call_id]: info,
        },
        toolCalls: {
          ...state.toolCalls,
          [sessionId]: hasToolCall ? currentToolCalls : [...currentToolCalls, info.tool_call_id],
        },
      };
    });
  },

  updateToolCard: (_sessionId, toolCallId, updates) => {
    set((state) => {
      const current = state.pendingConfirms[toolCallId];
      if (!current) return {};

      // Check if any updates actually changed
      let hasChange = false;
      for (const key in updates) {
        if (current[key as keyof ToolCallInfo] !== updates[key as keyof ToolCallInfo]) {
          hasChange = true;
          break;
        }
      }
      if (!hasChange) return {};

      return {
        pendingConfirms: {
          ...state.pendingConfirms,
          [toolCallId]: {
            ...current,
            ...updates,
          },
        },
      };
    });
  },

  removeToolCard: (_sessionId, toolCallId) => {
    set((state) => {
      if (!state.pendingConfirms[toolCallId]) return {};
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
