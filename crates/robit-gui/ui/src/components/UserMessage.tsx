import { User } from "lucide-react";

interface UserMessageProps {
  content: string;
}

export function UserMessage({ content }: UserMessageProps) {
  return (
    <div className="flex gap-3 px-4 py-3">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10">
        <User className="h-4 w-4 text-primary" />
      </div>
      <div className="flex-1 pt-1">
        <div className="text-sm font-medium text-muted-foreground mb-1">You</div>
        <div className="text-sm whitespace-pre-wrap">{content}</div>
      </div>
    </div>
  );
}
