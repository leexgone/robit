import { useRef, useState } from "react";
import { MessageSquare, MoreHorizontal, Pencil, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import type { SessionInfo } from "@/lib/types";
import { useStore } from "@/lib/store";
import { deleteSession, renameSession, switchSession } from "@/lib/commands";

interface SessionItemProps {
  session: SessionInfo;
}

export function SessionItem({ session }: SessionItemProps) {
  const activeSessionId = useStore((s) => s.activeSessionId);
  const setActiveSession = useStore((s) => s.setActiveSession);
  const setMessages = useStore((s) => s.setMessages);
  const removeSession = useStore((s) => s.removeSession);
  const updateSessionTitle = useStore((s) => s.updateSessionTitle);

  const [isEditing, setIsEditing] = useState(false);
  const [editTitle, setEditTitle] = useState(session.title);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const isActive = session.id === activeSessionId;

  const handleClick = async () => {
    if (isActive || isEditing) return;
    try {
      const msgs = await switchSession(session.id);
      setActiveSession(session.id);
      setMessages(session.id, msgs);
    } catch (e) {
      console.error("Failed to switch session:", e);
    }
  };

  const handleRename = async () => {
    if (!editTitle.trim()) return;
    try {
      await renameSession(session.id, editTitle.trim());
      updateSessionTitle(session.id, editTitle.trim());
    } catch (e) {
      console.error("Failed to rename session:", e);
    }
    setIsEditing(false);
  };

  const handleDelete = async () => {
    try {
      await deleteSession(session.id);
      removeSession(session.id);
      if (isActive) {
        setActiveSession(null);
      }
    } catch (e) {
      console.error("Failed to delete session:", e);
    }
    setShowDeleteDialog(false);
  };

  const handleInputBlur = () => {
    handleRename();
  };

  const handleInputKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      handleRename();
    }
    if (e.key === "Escape") {
      setEditTitle(session.title);
      setIsEditing(false);
    }
  };

  return (
    <>
      <div
        onClick={handleClick}
        className={`
          group flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer text-sm transition-colors
          ${
            isActive
              ? "bg-accent text-accent-foreground"
              : "hover:bg-accent/50 text-foreground"
          }
        `}
      >
        <MessageSquare className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        {isEditing ? (
          <input
            ref={inputRef}
            value={editTitle}
            onChange={(e) => setEditTitle(e.target.value)}
            onBlur={handleInputBlur}
            onKeyDown={handleInputKeyDown}
            className="flex-1 min-w-0 bg-transparent border border-input rounded px-1.5 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
            onClick={(e) => e.stopPropagation()}
            autoFocus
          />
        ) : (
          <span className="flex-1 truncate" title={session.title}>{session.title}</span>
        )}
        {session.status === "running" && (
          <span className="h-2 w-2 rounded-full bg-green-500 animate-pulse shrink-0" />
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild onClick={(e) => e.stopPropagation()}>
            <Button variant="ghost" size="icon" className="h-5 w-5 opacity-0 group-hover:opacity-100 shrink-0">
              <MoreHorizontal className="h-3 w-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-36">
            <DropdownMenuItem
              onClick={(e) => {
                e.stopPropagation();
                setIsEditing(true);
              }}
            >
              <Pencil className="h-3.5 w-3.5 mr-2" />
              Rename
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(e) => {
                e.stopPropagation();
                setShowDeleteDialog(true);
              }}
              className="text-destructive"
            >
              <Trash2 className="h-3.5 w-3.5 mr-2" />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <AlertDialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete Session</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to delete "{session.title}"?
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} className="bg-destructive hover:bg-destructive/90">
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
