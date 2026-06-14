import { useState, useRef, useEffect } from "react";
import { Send } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useStore } from "@/lib/store";
import { sendMessage } from "@/lib/commands";
import type { MessageData } from "@/lib/types";

export function InputArea() {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const agentStatusStore = useStore((s) => s.agentStatus);
  const setAgentStatus = useStore((s) => s.setAgentStatus);
  const setMessages = useStore((s) => s.setMessages);
  const messagesStore = useStore((s) => s.messages);

  const [value, setValue] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const agentStatus = activeSessionId ? agentStatusStore[activeSessionId] || "idle" : "idle";
  const isBusy = agentStatus === "running";

  // Auto-focus when value is cleared and not busy
  useEffect(() => {
    if (value === "" && textareaRef.current && !isBusy) {
      setTimeout(() => {
        textareaRef.current?.focus();
      }, 0);
    }
  }, [value, isBusy]);

  const handleSend = async () => {
    const trimmed = value.trim();
    if (!trimmed || !activeSessionId || isBusy) return;

    // Immediately add user message to local state
    const currentMessages = messagesStore[activeSessionId] || [];
    const userMessage: MessageData = {
      id: Date.now(),
      role: "user",
      content: trimmed,
      created_at: new Date().toISOString(),
    };
    setMessages(activeSessionId, [...currentMessages, userMessage]);

    // Clear value, useEffect will handle focus
    setValue("");
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
    }

    try {
      setAgentStatus(activeSessionId, "running");
      await sendMessage(activeSessionId, trimmed);
    } catch (e) {
      console.error("Failed to send message:", e);
      setAgentStatus(activeSessionId, "ready");
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleInput = () => {
    const el = textareaRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = Math.min(el.scrollHeight, 200) + "px";
    }
  };

  return (
    <div className="border-t p-3 shrink-0 min-w-0">
      <div className="flex items-end gap-2 min-w-0">
        <textarea
          ref={textareaRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          onInput={handleInput}
          placeholder="Type a message... (Enter to send, Shift+Enter for new line)"
          disabled={isBusy || !activeSessionId}
          rows={1}
          className="flex-1 min-w-0 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 min-h-[36px] max-h-[200px]"
        />
        <Button
          size="icon"
          onClick={handleSend}
          disabled={isBusy || !value.trim() || !activeSessionId}
          className="shrink-0"
        >
          <Send className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
