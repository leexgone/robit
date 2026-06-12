import { Bot } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";

interface AssistantMessageProps {
  content: string;
  isStreaming?: boolean;
}

export function AssistantMessage({ content, isStreaming }: AssistantMessageProps) {
  return (
    <div className="flex justify-start px-4 py-3">
      <div className="flex items-start gap-3 max-w-[85%]">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent mt-0.5">
          <Bot className="h-4 w-4 text-accent-foreground" />
        </div>
        <div className="flex flex-col items-start min-w-0">
          <div className="text-xs font-medium text-muted-foreground mb-1">Robit</div>
          <div className="bg-accent text-accent-foreground rounded-2xl rounded-tl-sm px-4 py-3 max-w-full min-w-0">
            <div className="markdown-body text-sm">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                components={{
                  // 代码块 — 使用 SyntaxHighlighter 带语法高亮
                  code({ className, children, ...props }) {
                    const match = /language-(\w+)/.exec(className || "");
                    const codeStr = String(children).replace(/\n$/, "");
                    const isInline = !match && !codeStr.includes("\n");

                    if (isInline) {
                      return (
                        <code className={className} {...props}>
                          {children}
                        </code>
                      );
                    }

                    return (
                      <SyntaxHighlighter
                        style={oneDark}
                        language={match ? match[1] : "text"}
                        PreTag="div"
                        customStyle={{
                          margin: 0,
                          borderRadius: "0.5rem",
                          fontSize: "0.85rem",
                        }}
                      >
                        {codeStr}
                      </SyntaxHighlighter>
                    );
                  },

                  // 标题
                  h1({ children }) {
                    return <h1>{children}</h1>;
                  },
                  h2({ children }) {
                    return <h2>{children}</h2>;
                  },
                  h3({ children }) {
                    return <h3>{children}</h3>;
                  },
                  h4({ children }) {
                    return <h4>{children}</h4>;
                  },
                  h5({ children }) {
                    return <h5>{children}</h5>;
                  },
                  h6({ children }) {
                    return <h6>{children}</h6>;
                  },

                  // 段落
                  p({ children }) {
                    return <p>{children}</p>;
                  },

                  // 列表
                  ul({ children }) {
                    return <ul>{children}</ul>;
                  },
                  ol({ children }) {
                    return <ol>{children}</ol>;
                  },
                  li({ children }) {
                    return <li>{children}</li>;
                  },

                  // 引用块
                  blockquote({ children }) {
                    return <blockquote>{children}</blockquote>;
                  },

                  // 链接 — 外部链接新窗口打开
                  a({ href, children }) {
                    return (
                      <a href={href} target="_blank" rel="noopener noreferrer">
                        {children}
                      </a>
                    );
                  },

                  // 水平分割线
                  hr() {
                    return <hr />;
                  },

                  // 图片
                  img({ src, alt }) {
                    return <img src={src} alt={alt} />;
                  },

                  // 加粗
                  strong({ children }) {
                    return <strong>{children}</strong>;
                  },

                  // 斜体
                  em({ children }) {
                    return <em>{children}</em>;
                  },

                  // 删除线 (GFM)
                  del({ children }) {
                    return <del>{children}</del>;
                  },

                  // 表格 (GFM)
                  table({ children }) {
                    return (
                      <div className="overflow-x-auto my-2">
                        <table>{children}</table>
                      </div>
                    );
                  },
                  thead({ children }) {
                    return <thead>{children}</thead>;
                  },
                  tbody({ children }) {
                    return <tbody>{children}</tbody>;
                  },
                  tr({ children }) {
                    return <tr>{children}</tr>;
                  },
                  th({ children }) {
                    return <th>{children}</th>;
                  },
                  td({ children }) {
                    return <td>{children}</td>;
                  },

                  // 换行
                  br() {
                    return <br />;
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
