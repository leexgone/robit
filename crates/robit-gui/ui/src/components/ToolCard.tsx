import { Wrench, Check, X, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ToolCallInfo } from "@/lib/types";
import { confirmTool } from "@/lib/commands";
import { useStore } from "@/lib/store";

interface ToolCardProps {
  info: ToolCallInfo;
}

export function ToolCard({ info }: ToolCardProps) {
  const activeSessionId = useStore((s) => s.activeSessionId);

  const handleConfirm = async (approved: boolean) => {
    if (!activeSessionId) return;
    try {
      await confirmTool(activeSessionId, info.tool_call_id, approved);
    } catch (e) {
      console.error("Failed to confirm tool:", e);
    }
  };

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

  return (
    <div className="mx-4 my-2 border rounded-lg overflow-hidden bg-card min-w-0">
      <div className="flex items-center gap-2 px-3 py-2 bg-secondary/50 border-b text-xs min-w-0">
        {statusIcon()}
        <span className="font-medium truncate">🔧 {info.name}</span>
        <span className="text-muted-foreground shrink-0">{statusText()}</span>
      </div>
      <div className="p-3 min-w-0">
        <div className="text-xs text-muted-foreground mb-1">Arguments:</div>
        <pre className="text-xs bg-muted p-2 rounded overflow-x-auto whitespace-pre-wrap max-w-full">
          {info.arguments}
        </pre>
        {info.output && (
          <>
            <div className="text-xs text-muted-foreground mt-2 mb-1">Output:</div>
            <pre className="text-xs bg-muted p-2 rounded overflow-x-auto max-h-40 overflow-y-auto whitespace-pre-wrap max-w-full">
              {info.output}
            </pre>
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
