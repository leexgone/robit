import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { StatusBar } from "@/components/StatusBar";
import { SessionSidebar } from "@/components/SessionSidebar";
import { ChatPanel } from "@/components/ChatPanel";
import { useStore } from "@/lib/store";
import { listSessions, getConfig } from "@/lib/commands";
import type { UiEvent } from "@/lib/types";

function App() {
  const setSessions = useStore((s) => s.setSessions);
  const setConfig = useStore((s) => s.setConfig);
  const appendStreaming = useStore((s) => s.appendStreaming);
  const commitStreaming = useStore((s) => s.commitStreaming);
  const setAgentStatus = useStore((s) => s.setAgentStatus);
  const addToolCard = useStore((s) => s.addToolCard);
  const updateToolCard = useStore((s) => s.updateToolCard);

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
          addToolCard(sid, {
            tool_call_id: payload.tool_call_id,
            name: payload.name,
            arguments: payload.arguments,
            status: payload.requires_confirm
              ? "awaiting_confirmation"
              : "running",
            requires_confirm: payload.requires_confirm,
          });
          break;

        case "ToolCallResult":
          updateToolCard(sid, payload.tool_call_id, {
            status: payload.is_error ? "error" : "success",
            output: payload.content,
          });
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
  }, [appendStreaming, commitStreaming, setAgentStatus, addToolCard, updateToolCard]);

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
