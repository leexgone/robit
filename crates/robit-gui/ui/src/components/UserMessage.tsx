import { User, AlarmClock } from "lucide-react";
import { memo } from "react";

interface UserMessageProps {
  content: string;
  /** Origin marker; "scheduler" = a scheduled-task fire, not the user typing. */
  source?: string | null;
}

function UserMessageComponent({ content, source }: UserMessageProps) {
  const isScheduled = source === "scheduler";
  return (
    <div className="flex justify-end py-3 min-w-0">
      <div className="flex items-end gap-3 max-w-[min(80%,720px)] min-w-0">
        <div className="flex flex-col items-end min-w-0">
          <div className="text-xs font-medium text-muted-foreground mb-1">
            {isScheduled ? (
              <span className="inline-flex items-center gap-1">
                <AlarmClock className="h-3 w-3" /> 计划任务
              </span>
            ) : (
              "You"
            )}
          </div>
          <div className="bg-primary text-primary-foreground rounded-2xl rounded-tr-sm px-4 py-2 max-w-full min-w-0">
            <div className="text-sm whitespace-pre-wrap break-words min-w-0">{content}</div>
          </div>
        </div>
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10">
          {isScheduled ? (
            <AlarmClock className="h-4 w-4 text-primary" />
          ) : (
            <User className="h-4 w-4 text-primary" />
          )}
        </div>
      </div>
    </div>
  );
}

export const UserMessage = memo(UserMessageComponent, (prev, next) => {
  return prev.content === next.content && prev.source === next.source;
});
