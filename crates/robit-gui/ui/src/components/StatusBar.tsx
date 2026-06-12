import { Bot, Sparkles } from "lucide-react";
import { useStore } from "@/lib/store";
import { ThemeToggle } from "./ThemeToggle";

export function StatusBar() {
  const config = useStore((s) => s.config);

  return (
    <div className="flex items-center justify-between h-9 px-3 bg-secondary border-b text-xs text-muted-foreground shrink-0">
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-1.5">
          <Bot className="h-3.5 w-3.5" />
          <span className="font-medium text-foreground">
            robit v{config?.version || "0.1.0"}
          </span>
        </div>
        <span className="text-border">│</span>
        <span>{config?.model || "Loading..."}</span>
        <span className="text-border">│</span>
        <span>
          Tools: {config?.tools_enabled || 0}/{config?.tools_total || 0}
        </span>
      </div>
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1.5">
          <Sparkles className="h-3.5 w-3.5" />
          Skills: {config?.skills_enabled || 0}/{config?.skills_total || 0}
        </span>
        <ThemeToggle />
      </div>
    </div>
  );
}
