import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SessionItem } from "./SessionItem";
import { useStore } from "@/lib/store";
import { createSession } from "@/lib/commands";

export function SessionSidebar() {
  const sessions = useStore((s) => s.sessions);
  const sidebarWidth = useStore((s) => s.sidebarWidth);
  const setSidebarWidth = useStore((s) => s.setSidebarWidth);
  const addSession = useStore((s) => s.addSession);
  const setActiveSession = useStore((s) => s.setActiveSession);
  const config = useStore((s) => s.config);

  const handleCreateSession = async () => {
    try {
      const model = config?.model || "deepseek/deepseek-chat";
      const session = await createSession(model);
      addSession(session);
      setActiveSession(session.id);
    } catch (e) {
      console.error("Failed to create session:", e);
    }
  };

  const handleResize = (e: React.MouseEvent) => {
    const startX = e.clientX;
    const startWidth = sidebarWidth;

    const onMouseMove = (e: MouseEvent) => {
      const delta = e.clientX - startX;
      const newWidth = Math.min(400, Math.max(160, startWidth + delta));
      setSidebarWidth(newWidth);
    };

    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    };

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  };

  return (
    <div className="flex shrink-0 h-full" style={{ width: sidebarWidth }}>
      <div className="flex flex-col flex-1 border-r min-w-0 h-full">
        <div className="flex items-center justify-between px-3 py-2 border-b shrink-0">
          <span className="text-xs font-medium text-muted-foreground">Sessions</span>
          <Button variant="ghost" size="icon" className="h-6 w-6" onClick={handleCreateSession}>
            <Plus className="h-3.5 w-3.5" />
          </Button>
        </div>
        <ScrollArea className="flex-1">
          <div className="p-2 space-y-0.5">
            {sessions.map((session) => (
              <SessionItem key={session.id} session={session} />
            ))}
            {sessions.length === 0 && (
              <p className="text-xs text-muted-foreground text-center py-8">
                No sessions. Click + to create one.
              </p>
            )}
          </div>
        </ScrollArea>
      </div>
      {/* Resize handle */}
      <div
        className="w-1 cursor-col-resize hover:bg-accent transition-colors shrink-0"
        onMouseDown={handleResize}
      />
    </div>
  );
}
