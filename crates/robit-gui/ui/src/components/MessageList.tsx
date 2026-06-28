import { useEffect, useRef, useCallback, memo, useMemo } from "react";
import { Bot, Loader2 } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useStore } from "@/lib/store";
import { UserMessage } from "./UserMessage";
import { AssistantMessage } from "./AssistantMessage";
import { ToolCard } from "./ToolCard";
import type { ToolCallInfo, MessageData } from "@/lib/types";

// Memoized message components
const MemoizedUserMessage = memo(UserMessage);
const MemoizedAssistantMessage = memo(AssistantMessage);
const MemoizedToolCard = memo(ToolCard);

function ThinkingIndicatorComponent() {
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

const ThinkingIndicator = memo(ThinkingIndicatorComponent);

// Individual message item component - memoized
interface MessageItemProps {
  msg: MessageData;
  pendingConfirms: Record<string, ToolCallInfo>;
}

const MessageItem = memo(function MessageItem({ msg, pendingConfirms }: MessageItemProps) {
  // Helper to parse tool_info from message - memoized
  const toolInfo = useMemo((): ToolCallInfo | undefined => {
    if (!msg.tool_info) {
      // Fallback: build minimal tool info from available fields
      if (msg.tool_call_id) {
        return {
          tool_call_id: msg.tool_call_id,
          name: msg.tool_name || "",
          arguments: msg.content,
          status: "success",
          requires_confirm: false,
        };
      }
      return undefined;
    }
    try {
      let parsed = typeof msg.tool_info === "string"
        ? JSON.parse(msg.tool_info)
        : msg.tool_info;

      // Ensure required fields exist (backward compatibility)
      if (parsed && typeof parsed === "object") {
        if (!parsed.name && msg.tool_name) {
          parsed.name = msg.tool_name;
        }
        if (!parsed.arguments && msg.content) {
          parsed.arguments = msg.content;
        }
        if (!parsed.status) {
          parsed.status = "success";
        }
        if (!parsed.requires_confirm) {
          parsed.requires_confirm = false;
        }
      }
      return parsed;
    } catch (e) {
      // Fallback if JSON parse fails
      if (msg.tool_call_id) {
        return {
          tool_call_id: msg.tool_call_id,
          name: msg.tool_name || "",
          arguments: msg.content,
          status: "success",
          requires_confirm: false,
        };
      }
      return undefined;
    }
  }, [msg.tool_info, msg.tool_call_id, msg.tool_name, msg.content]);

  // Prefer latest state from pendingConfirms
  const currentToolInfo = useMemo(() => {
    if (msg.tool_call_id && pendingConfirms[msg.tool_call_id]) {
      return pendingConfirms[msg.tool_call_id];
    }
    return toolInfo;
  }, [msg.tool_call_id, pendingConfirms, toolInfo]);

  if (msg.role === "user") {
    return <MemoizedUserMessage content={msg.content} />;
  }
  if (msg.role === "assistant") {
    return <MemoizedAssistantMessage content={msg.content} />;
  }
  if (msg.role === "tool" && currentToolInfo) {
    return <MemoizedToolCard info={currentToolInfo} />;
  }
  return null;
});

function MessageListComponent() {
  // Select only what we need with stable selectors
  const activeSessionId = useStore((s) => s.activeSessionId);
  const messages = useStore((s) => activeSessionId ? s.messages[activeSessionId] : null);
  const streamingBuffer = useStore((s) => activeSessionId ? s.streamingBuffer[activeSessionId] : null);
  const agentStatus = useStore((s) => activeSessionId ? s.agentStatus[activeSessionId] : null);
  const pendingConfirms = useStore((s) => s.pendingConfirms);

  const scrollAreaRef = useRef<HTMLDivElement>(null);
  const lastScrollHeightRef = useRef<number>(0);

  // Memoized values to avoid recalculation on every render
  const messageList = useMemo(() => messages || [], [messages]);
  const currentStreamingBuffer = useMemo(() => streamingBuffer || "", [streamingBuffer]);
  const currentAgentStatus = useMemo(() => agentStatus || "idle", [agentStatus]);

  // Determine if we should show the thinking indicator
  const showThinkingIndicator = useMemo(() => {
    if (currentAgentStatus !== "running") return false;
    if (currentStreamingBuffer) return false;

    const lastMsg = messageList[messageList.length - 1];
    if (!lastMsg) return true;

    if (lastMsg.role === "user") return true;

    if (lastMsg.role === "tool") {
      try {
        const info = typeof lastMsg.tool_info === "string"
          ? JSON.parse(lastMsg.tool_info)
          : lastMsg.tool_info;
        return info?.status === "success" || info?.status === "error";
      } catch {
        return false;
      }
    }

    return false;
  }, [currentAgentStatus, currentStreamingBuffer, messageList]);

  // Auto-scroll function - debounced and smarter
  const scrollToBottom = useCallback(() => {
    const scrollAreaEl = scrollAreaRef.current;
    if (!scrollAreaEl) return;

    const viewport = scrollAreaEl.querySelector('[data-radix-scroll-area-viewport]') as HTMLElement | null;
    if (viewport) {
      const scrollHeight = viewport.scrollHeight;
      const scrollTop = viewport.scrollTop;
      const clientHeight = viewport.clientHeight;

      // Only auto-scroll if user is already near the bottom
      const isNearBottom = scrollHeight - scrollTop - clientHeight < 100;
      // Or if content just grew (new message/streaming update)
      const contentJustGrew = scrollHeight !== lastScrollHeightRef.current;

      if (isNearBottom || contentJustGrew) {
        viewport.scrollTop = scrollHeight;
        lastScrollHeightRef.current = scrollHeight;
      }
    }
  }, []);

  // Scroll when active session changes
  useEffect(() => {
    if (activeSessionId) {
      // Small delay to let content render first
      const timer = setTimeout(() => {
        scrollToBottom();
        lastScrollHeightRef.current = 0;
      }, 50);
      return () => clearTimeout(timer);
    }
  }, [activeSessionId, scrollToBottom]);

  // Scroll for new content - debounced
  useEffect(() => {
    const timer = setTimeout(scrollToBottom, 10);
    return () => clearTimeout(timer);
  }, [messageList.length, currentStreamingBuffer, showThinkingIndicator, scrollToBottom]);

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
        {/* Render messages with memoized components */}
        {messageList.map((msg) => (
          <MessageItem key={msg.id} msg={msg} pendingConfirms={pendingConfirms} />
        ))}

        {showThinkingIndicator && <ThinkingIndicator />}

        {currentStreamingBuffer && (
          <MemoizedAssistantMessage content={currentStreamingBuffer} isStreaming />
        )}
      </div>
    </ScrollArea>
  );
}

// Memoize the entire MessageList
export const MessageList = memo(MessageListComponent);
