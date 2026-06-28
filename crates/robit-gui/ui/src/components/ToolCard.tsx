import { Wrench, Check, X, Loader2, ChevronDown, ChevronRight, Copy, CheckCircle2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ToolCallInfo } from "@/lib/types";
import { confirmTool } from "@/lib/commands";
import { useStore } from "@/lib/store";
import { useState, useCallback } from "react";

interface ToolCardProps {
  info: ToolCallInfo;
}

// Configuration for output truncation
const MAX_OUTPUT_LINES = 50;
const MAX_OUTPUT_CHARS = 10000;

export function ToolCard({ info }: ToolCardProps) {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const [isOutputExpanded, setIsOutputExpanded] = useState(false);
  const [isArgsExpanded, setIsArgsExpanded] = useState(false);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied">("idle");

  const handleConfirm = async (approved: boolean) => {
    if (!activeSessionId) return;
    try {
      await confirmTool(activeSessionId, info.tool_call_id, approved);
    } catch (e) {
      console.error("Failed to confirm tool:", e);
    }
  };

  const handleCopy = useCallback(() => {
    if (!info.output) return;
    navigator.clipboard.writeText(info.output).then(() => {
      setCopyStatus("copied");
      setTimeout(() => setCopyStatus("idle"), 2000);
    });
  }, [info.output]);

  const statusIcon = () => {
    switch (info.status) {
      case "running":
        return <Loader2 className="h-4 w-4 animate-spin text-blue-500" />;
      case "success":
        return <Check className="h-4 w-4 text-green-500" />;
      case "error":
        return <X className="h-4 w-4 text-red-500" />;
      case "awaiting_confirmation":
        return <Wrench className="h-4 w-4 text-yellow-500" />;
    }
  };

  const statusText = () => {
    switch (info.status) {
      case "running":
        return "Running...";
      case "success":
        return "Completed";
      case "error":
        return "Error";
      case "awaiting_confirmation":
        return "Waiting for confirmation";
    }
  };

  // Truncate long output
  const shouldTruncateOutput = info.output && (
    info.output.length > MAX_OUTPUT_CHARS ||
    info.output.split("\n").length > MAX_OUTPUT_LINES
  );

  const displayOutput = shouldTruncateOutput && !isOutputExpanded && info.output
    ? truncateOutput(info.output)
    : info.output;

  // Truncate long arguments
  const shouldTruncateArgs = info.arguments && (
    info.arguments.length > 500 ||
    info.arguments.split("\n").length > 20
  );

  const displayArgs = shouldTruncateArgs && !isArgsExpanded
    ? truncateOutput(info.arguments, 500, 20)
    : info.arguments;

  return (
    <div className="my-2 border rounded-lg overflow-hidden bg-card min-w-0 max-w-full">
      <div className="flex items-center gap-2 px-3 py-2 bg-secondary/50 border-b text-xs min-w-0">
        {statusIcon()}
        <span className="font-medium truncate" title={info.name}>🔧 {info.name}</span>
        <span className="text-muted-foreground shrink-0" title={statusText()}>{statusText()}</span>
      </div>
      <div className="p-3 min-w-0">
        <div className="flex items-center justify-between mb-1">
          <div className="text-xs text-muted-foreground">Arguments:</div>
          {shouldTruncateArgs && (
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-xs"
              onClick={() => setIsArgsExpanded(!isArgsExpanded)}
            >
              {isArgsExpanded ? (
                <ChevronDown className="h-3 w-3 mr-1" />
              ) : (
                <ChevronRight className="h-3 w-3 mr-1" />
              )}
              {isArgsExpanded ? "Show less" : "Show more"}
            </Button>
          )}
        </div>
        <pre className="text-xs bg-muted p-2 rounded overflow-x-auto whitespace-pre-wrap max-w-full">
          {displayArgs}
        </pre>

        {info.output && (
          <>
            <div className="flex items-center justify-between mt-2 mb-1">
              <div className="text-xs text-muted-foreground">
                Output:
                {shouldTruncateOutput && (
                  <span className="ml-1 text-muted-foreground/70">
                    ({info.output.length.toLocaleString()} chars, {info.output.split("\n").length} lines)
                  </span>
                )}
              </div>
              <div className="flex gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 px-2 text-xs"
                  onClick={handleCopy}
                >
                  {copyStatus === "copied" ? (
                    <CheckCircle2 className="h-3 w-3 mr-1 text-green-500" />
                  ) : (
                    <Copy className="h-3 w-3 mr-1" />
                  )}
                  {copyStatus === "copied" ? "Copied" : "Copy"}
                </Button>
                {shouldTruncateOutput && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-xs"
                    onClick={() => setIsOutputExpanded(!isOutputExpanded)}
                  >
                    {isOutputExpanded ? (
                      <ChevronDown className="h-3 w-3 mr-1" />
                    ) : (
                      <ChevronRight className="h-3 w-3 mr-1" />
                    )}
                    {isOutputExpanded ? "Show less" : "Show more"}
                  </Button>
                )}
              </div>
            </div>
            <pre
              className={`text-xs bg-muted p-2 rounded overflow-x-auto whitespace-pre-wrap max-w-full ${
                !isOutputExpanded && shouldTruncateOutput ? "max-h-40 overflow-y-auto" : ""
              }`}
            >
              {displayOutput}
            </pre>
            {shouldTruncateOutput && !isOutputExpanded && (
              <div className="text-xs text-muted-foreground mt-1 text-center">
                Output truncated - click "Show more" to see full content
              </div>
            )}
          </>
        )}
      </div>
      {info.status === "awaiting_confirmation" && info.requires_confirm && (
        <div className="flex gap-2 px-3 py-2 border-t bg-secondary/30">
          <Button
            size="sm"
            variant="default"
            className="bg-green-600 hover:bg-green-700"
            onClick={() => handleConfirm(true)}
          >
            <Check className="h-3 w-3 mr-1" />
            Allow
          </Button>
          <Button
            size="sm"
            variant="destructive"
            onClick={() => handleConfirm(false)}
          >
            <X className="h-3 w-3 mr-1" />
            Deny
          </Button>
        </div>
      )}
    </div>
  );
}

function truncateOutput(output: string, maxChars: number = MAX_OUTPUT_CHARS, maxLines: number = MAX_OUTPUT_LINES): string {
  const lines = output.split("\n");

  if (lines.length <= maxLines && output.length <= maxChars) {
    return output;
  }

  // Try to truncate by lines first
  if (lines.length > maxLines) {
    const truncatedLines = lines.slice(0, maxLines);
    const result = truncatedLines.join("\n");

    // Check if still too long
    if (result.length <= maxChars) {
      return result + `\n\n[... truncated ${lines.length - maxLines} more lines ...]`;
    }
  }

  // Truncate by characters
  const charTruncated = output.slice(0, maxChars);
  const lastNewline = charTruncated.lastIndexOf("\n");
  const cleanTruncate = lastNewline > maxChars * 0.7 ? charTruncated.slice(0, lastNewline) : charTruncated;

  return cleanTruncate + `\n\n[... truncated ${(output.length - cleanTruncate.length).toLocaleString()} more characters ...]`;
}
