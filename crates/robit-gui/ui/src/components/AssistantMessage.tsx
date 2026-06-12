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
    <div className="flex justify-start px-4 py-3">
      <div className="flex items-end gap-3 max-w-[80%]">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent">
          <Bot className="h-4 w-4 text-accent-foreground" />
        </div>
        <div className="flex flex-col items-start">
          <div className="text-xs font-medium text-muted-foreground mb-1">Robit</div>
          <div className="bg-accent text-accent-foreground rounded-2xl rounded-tl-sm px-4 py-2 max-w-full">
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
                  table({ children }) {
                    return (
                      <div className="overflow-x-auto my-4">
                        <table className="border-collapse border border-border w-full text-sm">{children}</table>
                      </div>
                    );
                  },
                  thead({ children }) {
                    return <thead className="bg-muted">{children}</thead>;
                  },
                  tbody({ children }) {
                    return <tbody className="divide-y divide-border">{children}</tbody>;
                  },
                  tr({ children }) {
                    return <tr>{children}</tr>;
                  },
                  th({ children }) {
                    return <th className="border border-border px-4 py-2 text-left font-semibold">{children}</th>;
                  },
                  td({ children }) {
                    return <td className="border border-border px-4 py-2">{children}</td>;
                  },
                }}
              >
                {content || (isStreaming ? "▊" : "")}
              </ReactMarkdown>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
