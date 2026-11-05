import { Brain, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

interface ThinkingPanelProps {
  /** Accumulated thinking text (display-only; never persisted). */
  content: string;
  /** Whether the agent turn is still running. */
  active: boolean;
  /** Whether answer content (text or tool activity) has started for this turn. */
  hasAnswer: boolean;
}

/**
 * Collapsible "thinking process" panel rendered above the streaming answer
 * bubble while a thinking model emits `reasoning_content` increments.
 *
 * State machine:
 * - thinking  (active, no answer yet): auto-expanded, streaming grey text;
 * - writing   (answer started): auto-collapses once to a header row;
 * - done      (turn complete): stays collapsed, click to review until the
 *   next user message clears the buffer.
 */
function ThinkingPanelComponent({ content, active, hasAnswer }: ThinkingPanelProps) {
  const [expanded, setExpanded] = useState(true);
  const bodyRef = useRef<HTMLDivElement>(null);
  const collapsedOnceRef = useRef(false);

  // Auto-collapse exactly once, on the rising edge of "answer started" or
  // "turn finished" — a manual re-expand by the user is never fought.
  const collapseSignal = hasAnswer || !active;
  useEffect(() => {
    if (collapseSignal && !collapsedOnceRef.current) {
      collapsedOnceRef.current = true;
      setExpanded(false);
    }
  }, [collapseSignal]);

  // Follow the tail while thinking text streams in.
  useEffect(() => {
    if (expanded && active && !hasAnswer && bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [content, expanded, active, hasAnswer]);

  return (
    <div className="flex justify-start py-1 min-w-0">
      <div className="w-full max-w-full min-w-0 pl-11">
        <div className="rounded-lg border border-border/60 bg-muted/30 overflow-hidden">
          {/* Header row */}
          <button
            onClick={() => setExpanded((v) => !v)}
            className="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors"
          >
            <Brain className="h-3.5 w-3.5 shrink-0" />
            <span className="font-medium">思考过程</span>
            {active && !hasAnswer && (
              <Loader2 className="h-3 w-3 animate-spin shrink-0" />
            )}
            <span className="ml-auto flex items-center gap-1 shrink-0">
              {!expanded && (
                <span className="text-[10px] text-muted-foreground/70">
                  {content.length.toLocaleString()} 字
                </span>
              )}
              {expanded ? (
                <ChevronDown className="h-3 w-3" />
              ) : (
                <ChevronRight className="h-3 w-3" />
              )}
            </span>
          </button>

          {/* Body: plain text (thinking output is not markdown-rendered to
              avoid half-formed syntax flicker while streaming). */}
          {expanded && (
            <div
              ref={bodyRef}
              className="px-3 pb-2 max-h-64 overflow-y-auto text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap break-words select-text"
            >
              {content}
              {active && !hasAnswer && <span className="opacity-60">▊</span>}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export const ThinkingPanel = memo(
  ThinkingPanelComponent,
  (prev, next) =>
    prev.content === next.content &&
    prev.active === next.active &&
    prev.hasAnswer === next.hasAnswer
);
