import { Bot } from "lucide-react";
import ReactMarkdown from "react-markdown";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";

interface AssistantMessageProps {
  content: string;
  isStreaming?: boolean;
}

export function AssistantMessage({ content, isStreaming }: AssistantMessageProps) {
  return (
    <div className="flex gap-3 px-4 py-3">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent">
        <Bot className="h-4 w-4 text-accent-foreground" />
      </div>
      <div className="flex-1 pt-1 min-w-0">
        <div className="text-sm font-medium text-muted-foreground mb-1">Robit</div>
        <div className="prose prose-sm dark:prose-invert max-w-none text-sm">
          <ReactMarkdown
            components={{
              code({ className, children, ...props }) {
                const match = /language-(\w+)/.exec(className || "");
                const codeStr = String(children).replace(/\n$/, "");
                if (match) {
                  return (
                    <SyntaxHighlighter style={oneDark} language={match[1]} PreTag="div">
                      {codeStr}
                    </SyntaxHighlighter>
                  );
                }
                return (
                  <code className={className} {...props}>
                    {children}
                  </code>
                );
              },
            }}
          >
            {content || (isStreaming ? "▊" : "")}
          </ReactMarkdown>
        </div>
      </div>
    </div>
  );
}
