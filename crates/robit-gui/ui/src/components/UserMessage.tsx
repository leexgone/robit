import { User } from "lucide-react";

interface UserMessageProps {
  content: string;
}

export function UserMessage({ content }: UserMessageProps) {
  return (
    <div className="flex justify-end px-4 py-3">
      <div className="flex items-end gap-3 max-w-[80%]">
        <div className="flex flex-col items-end">
          <div className="text-xs font-medium text-muted-foreground mb-1">You</div>
          <div className="bg-primary text-primary-foreground rounded-2xl rounded-tr-sm px-4 py-2 max-w-full">
            <div className="text-sm whitespace-pre-wrap break-words">{content}</div>
          </div>
        </div>
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10">
          <User className="h-4 w-4 text-primary" />
        </div>
      </div>
    </div>
  );
}
