import { useEffect, useRef, useCallback } from "react";
import { Bot, Loader2 } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useStore } from "@/lib/store";
import { UserMessage } from "./UserMessage";
import { AssistantMessage } from "./AssistantMessage";
import { ToolCard } from "./ToolCard";
import type { ToolCallInfo } from "@/lib/types";

function ThinkingIndicator() {
  return (
    <div className="flex justify-start py-3 min-w-0">
      <div className="flex items-start gap-3 max-w-full min-w-0">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent mt-0.5">
          <Bot className="h-4 w-4 text-accent-foreground" />
        </div>
        <div className="flex flex-col items-start min-w-0">
          <div className="text-xs font-medium text-muted-foreground mb-1">Robit</div>
          <div className="flex items-center gap-2 bg-accent text-accent-foreground rounded-2xl rounded-tl-sm px-4 py-3 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" />
            <span>Robit is thinking...</span>
          </div>
        </div>
      </div>
    </div>
  );
}

export function MessageList() {
  // Select only top-level stable references
  const activeSessionId = useStore((s) => s.activeSessionId);
  const messagesStore = useStore((s) => s.messages);
  const streamingBufferStore = useStore((s) => s.streamingBuffer);
  const pendingConfirms = useStore((s) => s.pendingConfirms);
  const agentStatusStore = useStore((s) => s.agentStatus);

  const scrollAreaRef = useRef<HTMLDivElement>(null);

  // Derive values without creating new references in selectors
  const messages = activeSessionId ? messagesStore[activeSessionId] || [] : [];
  const streamingBuffer = activeSessionId ? streamingBufferStore[activeSessionId] || "" : "";
  const agentStatus = activeSessionId ? agentStatusStore[activeSessionId] || "idle" : "idle";

  // Helper to parse tool_info from message
  const parseToolInfo = (msg: any): ToolCallInfo | undefined => {
    if (!msg.tool_info) return undefined;
    try {
      const info: ToolCallInfo = typeof msg.tool_info === "string"
        ? JSON.parse(msg.tool_info)
        : msg.tool_info;
      return info;
    } catch (e) {
      return undefined;
    }
  };

  const lastMessage = messages[messages.length - 1];
  const lastToolInfo = lastMessage?.role === "tool" ? parseToolInfo(lastMessage) : undefined;
  const showThinkingIndicator = agentStatus === "running" && !streamingBuffer && (
    lastMessage?.role === "user" ||
    (lastMessage?.role === "tool" &&
      (lastToolInfo?.status === "success" || lastToolInfo?.status === "error"))
  );

  // Auto-scroll function
  const scrollToBottom = useCallback(() => {
    // Find ScrollArea viewport - shadcn/ui's ScrollArea renders viewport with class "[data-radix-scroll-area-viewport]"
    const scrollAreaEl = scrollAreaRef.current;
    if (!scrollAreaEl) return;

    const viewport = scrollAreaEl.querySelector('[data-radix-scroll-area-viewport]') as HTMLElement | null;
    if (viewport) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }, []);

  // Scroll when active session changes (opening history)
  useEffect(() => {
    scrollToBottom();
  }, [activeSessionId, scrollToBottom]);

  // Scroll when messages, streaming buffer, or thinking indicator change
  useEffect(() => {
    scrollToBottom();
  }, [messages.length, streamingBuffer, showThinkingIndicator, scrollToBottom]);

  if (!activeSessionId) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center text-muted-foreground">
        <div className="text-center">
          <p className="text-lg mb-2">Robit AI Automaton Agent</p>
          <p className="text-sm">Select a session or create a new one to get started</p>
        </div>
      </div>
    );
  }

  return (
    <ScrollArea className="flex-1 min-h-0 min-w-0" ref={scrollAreaRef}>
      <div className="mx-auto w-full max-w-6xl px-4 py-2 min-w-0">
        {messages.map((msg) => {
          if (msg.role === "user") {
            return <UserMessage key={msg.id} content={msg.content} />;
          }
          if (msg.role === "assistant") {
            return <AssistantMessage key={msg.id} content={msg.content} />;
          }
          if (msg.role === "tool") {
            // Prefer latest state from pendingConfirms, fall back to stored tool_info
            let info: ToolCallInfo | undefined;
            if (msg.tool_call_id && pendingConfirms[msg.tool_call_id]) {
              info = pendingConfirms[msg.tool_call_id];
            } else {
              info = parseToolInfo(msg);
            }
            if (info) {
              return <ToolCard key={msg.id} info={info} />;
            }
          }
          return null;
        })}

        {showThinkingIndicator && <ThinkingIndicator />}

        {streamingBuffer && (
          <AssistantMessage content={streamingBuffer} isStreaming />
        )}
      </div>
    </ScrollArea>
  );
}
