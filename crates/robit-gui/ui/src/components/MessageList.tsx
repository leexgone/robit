import { useEffect, useRef } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useStore } from "@/lib/store";
import { UserMessage } from "./UserMessage";
import { AssistantMessage } from "./AssistantMessage";
import { ToolCard } from "./ToolCard";

export function MessageList() {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const messages = useStore((s) =>
    activeSessionId ? s.messages[activeSessionId] || [] : []
  );
  const streamingBuffer = useStore((s) =>
    activeSessionId ? s.streamingBuffer[activeSessionId] || "" : ""
  );
  const pendingConfirms = useStore((s) => s.pendingConfirms);
  const viewportRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on new content
  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }, [messages, streamingBuffer, pendingConfirms]);

  if (!activeSessionId) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground">
        <div className="text-center">
          <p className="text-lg mb-2">Robit AI Programming Agent</p>
          <p className="text-sm">Select a session or create a new one to get started</p>
        </div>
      </div>
    );
  }

  return (
    <ScrollArea className="flex-1">
      <div ref={viewportRef} className="py-2">
        {messages.map((msg) => {
          if (msg.role === "user") {
            return <UserMessage key={msg.id} content={msg.content} />;
          }
          if (msg.role === "assistant") {
            return <AssistantMessage key={msg.id} content={msg.content} />;
          }
          // Tool messages are rendered via pendingConfirms
          return null;
        })}

        {/* Streaming text (in-progress assistant response) */}
        {streamingBuffer && (
          <AssistantMessage content={streamingBuffer} isStreaming />
        )}

        {/* Tool cards for current turn */}
        {Object.values(pendingConfirms).map((info) => (
          <ToolCard key={info.tool_call_id} info={info} />
        ))}
      </div>
    </ScrollArea>
  );
}
