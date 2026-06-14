import { Bot, Folder } from "lucide-react";
import { useStore } from "@/lib/store";
import { ThemeToggle } from "./ThemeToggle";

function formatDisplayPath(path?: string): string {
  if (!path) return "";
  return path.replace(/^\\\\\?\\/, "");
}

export function StatusBar() {
  const config = useStore((s) => s.config);
  const workingDir = formatDisplayPath(config?.working_dir);

  return (
    <div className="flex items-center justify-between h-9 px-3 bg-secondary border-b text-xs text-muted-foreground shrink-0 min-w-0">
      <div className="flex items-center gap-3 min-w-0">
        <div className="flex items-center gap-1.5 shrink-0">
          <Bot className="h-3.5 w-3.5" />
          <span className="font-medium text-foreground">
            robit v{config?.version || "0.1.0"}
          </span>
        </div>
        <span className="text-border shrink-0">│</span>
        <span className="truncate max-w-[18vw]" title={config?.model || ""}>{config?.model || "Loading..."}</span>
        <span className="text-border shrink-0">│</span>
        <span className="shrink-0">
          Tools: {config?.tools_enabled || 0}/{config?.tools_total || 0}
        </span>
        <span className="text-border shrink-0 hidden sm:inline">│</span>
        <span className="shrink-0 hidden sm:inline">
          Skills: {config?.skills_enabled || 0}/{config?.skills_total || 0}
        </span>
        <span className="text-border shrink-0">│</span>
        <div className="flex items-center gap-1.5 min-w-0" title={workingDir}>
          <Folder className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate max-w-[40vw]">
            {workingDir || "Loading path..."}
          </span>
        </div>
      </div>
      <div className="shrink-0">
        <ThemeToggle />
      </div>
    </div>
  );
}
