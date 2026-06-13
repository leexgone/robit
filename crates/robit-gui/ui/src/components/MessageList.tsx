import { useEffect, useRef, useCallback } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useStore } from "@/lib/store";
import { UserMessage } from "./UserMessage";
import { AssistantMessage } from "./AssistantMessage";
import { ToolCard } from "./ToolCard";
import type { ToolCallInfo } from "@/lib/types";

export function MessageList() {
  // Select only top-level stable references
  const activeSessionId = useStore((s) => s.activeSessionId);
  const messagesStore = useStore((s) => s.messages);
  const streamingBufferStore = useStore((s) => s.streamingBuffer);
  const pendingConfirms = useStore((s) => s.pendingConfirms);

  const scrollAreaRef = useRef<HTMLDivElement>(null);

  // Derive values without creating new references in selectors
  const messages = activeSessionId ? messagesStore[activeSessionId] || [] : [];
  const streamingBuffer = activeSessionId ? streamingBufferStore[activeSessionId] || "" : "";

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

  // Scroll when messages or streaming buffer change
  useEffect(() => {
    scrollToBottom();
  }, [messages.length, streamingBuffer, scrollToBottom]);

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

  if (!activeSessionId) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground">
        <div className="text-center">
          <p className="text-lg mb-2">Robit AI Automaton Agent</p>
          <p className="text-sm">Select a session or create a new one to get started</p>
        </div>
      </div>
    );
  }

  return (
    <ScrollArea className="flex-1" ref={scrollAreaRef}>
      <div className="py-2">
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

        {streamingBuffer && (
          <AssistantMessage content={streamingBuffer} isStreaming />
        )}
      </div>
    </ScrollArea>
  );
}
