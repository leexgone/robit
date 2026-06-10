import { MessageList } from "./MessageList";
import { InputArea } from "./InputArea";

export function ChatPanel() {
  return (
    <div className="flex-1 flex flex-col min-w-0 h-full">
      <MessageList />
      <InputArea />
    </div>
  );
}
