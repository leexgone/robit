import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { StatusBar } from "@/components/StatusBar";
import { SessionSidebar } from "@/components/SessionSidebar";
import { ChatPanel } from "@/components/ChatPanel";
import { useStore } from "@/lib/store";
import { listSessions, getConfig } from "@/lib/commands";
import type { UiEvent, MessageData, ToolCallInfo } from "@/lib/types";

function App() {
  const setSessions = useStore((s) => s.setSessions);
  const setConfig = useStore((s) => s.setConfig);
  const appendStreaming = useStore((s) => s.appendStreaming);
  const commitStreaming = useStore((s) => s.commitStreaming);
  const setAgentStatus = useStore((s) => s.setAgentStatus);
  const addToolCard = useStore((s) => s.addToolCard);
  const updateToolCard = useStore((s) => s.updateToolCard);
  const messagesStore = useStore((s) => s.messages);
  const setMessages = useStore((s) => s.setMessages);

  // Initialize app data
  useEffect(() => {
    const init = async () => {
      try {
        const [sessions, config] = await Promise.all([
          listSessions(),
          getConfig(),
        ]);
        setSessions(sessions);
        setConfig(config);
      } catch (e) {
        console.error("Failed to initialize app:", e);
      }
    };

    init();
  }, []);

  // Listen for Tauri events from Rust backend
  useEffect(() => {
    const unlisten = listen("agent-event", (event) => {
      const payload = event.payload as UiEvent;
      const sid = payload.session_id;

      switch (payload.type) {
        case "TextDelta":
          appendStreaming(sid, payload.delta);
          break;

        case "ToolCallRequested":
          // Create tool message and add to history
          const toolInfoRequested: ToolCallInfo = {
            tool_call_id: payload.tool_call_id,
            name: payload.name,
            arguments: payload.arguments,
            status: payload.requires_confirm
              ? "awaiting_confirmation"
              : "running",
            requires_confirm: payload.requires_confirm,
          };
          addToolCard(sid, toolInfoRequested);

          // Also add as a tool message
          const toolMsg: MessageData = {
            id: Date.now(),
            role: "tool",
            content: "",
            tool_call_id: payload.tool_call_id,
            tool_name: payload.name,
            tool_info: toolInfoRequested,
            created_at: new Date().toISOString(),
          };
          const currentMsgs = messagesStore[sid] || [];
          setMessages(sid, [...currentMsgs, toolMsg]);
          break;

        case "ToolCallResult":
          updateToolCard(sid, payload.tool_call_id, {
            status: payload.is_error ? "error" : "success",
            output: payload.content,
          });

          // Also update the tool message in history
          const msgsToUpdate = messagesStore[sid] || [];
          const updatedMsgs = msgsToUpdate.map(msg => {
            if (msg.tool_call_id === payload.tool_call_id && msg.tool_info) {
              return {
                ...msg,
                tool_info: {
                  ...msg.tool_info,
                  status: (payload.is_error ? "error" : "success") as ToolCallInfo["status"],
                  output: payload.content,
                },
              };
            }
            return msg;
          });
          setMessages(sid, updatedMsgs);
          break;

        case "TurnComplete":
          commitStreaming(sid);
          setAgentStatus(sid, "ready");
          break;

        case "Error":
          console.error("Agent error:", payload.message);
          setAgentStatus(sid, "ready");
          break;

        case "SkillTriggered":
          // Could show a toast notification here
          break;
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appendStreaming, commitStreaming, setAgentStatus, addToolCard, updateToolCard, messagesStore, setMessages]);

  return (
    <div className="h-screen flex flex-col bg-background text-foreground">
      <StatusBar />
      <div className="flex flex-1 overflow-hidden">
        <SessionSidebar />
        <ChatPanel />
      </div>
    </div>
  );
}

export { App };
