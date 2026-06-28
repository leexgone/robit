import { Bot, ChevronDown, ChevronRight } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
import { memo, useState, useMemo } from "react";

interface AssistantMessageProps {
  content: string;
  isStreaming?: boolean;
}

// Thresholds for content truncation
const MAX_CONTENT_LINES = 200;
const MAX_CONTENT_CHARS = 50000;

function AssistantMessageComponent({ content, isStreaming }: AssistantMessageProps) {
  const [isExpanded, setIsExpanded] = useState(true);

  // Check if content is large and should be truncatable
  const contentStats = useMemo(() => {
    const lines = content.split("\n").length;
    return {
      lines,
      chars: content.length,
      isLarge: lines > MAX_CONTENT_LINES || content.length > MAX_CONTENT_CHARS,
    };
  }, [content.length]);

  // Determine what content to render
  const displayContent = useMemo(() => {
    if (!contentStats.isLarge || isExpanded) {
      return content;
    }
    return truncateContent(content);
  }, [content, contentStats.isLarge, isExpanded]);

  return (
    <div className="flex justify-start py-3 min-w-0">
      <div className="flex items-start gap-3 max-w-full min-w-0">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-accent mt-0.5">
          <Bot className="h-4 w-4 text-accent-foreground" />
        </div>
        <div className="flex flex-col items-start min-w-0 w-full">
          <div className="text-xs font-medium text-muted-foreground mb-1 flex items-center gap-2">
            Robit
            {contentStats.isLarge && (
              <span className="text-[10px] text-muted-foreground/70">
                ({contentStats.lines.toLocaleString()} lines, {contentStats.chars.toLocaleString()} chars)
              </span>
            )}
          </div>
          <div className="bg-accent text-accent-foreground rounded-2xl rounded-tl-sm px-4 py-3 max-w-full min-w-0 overflow-hidden w-full">
            {contentStats.isLarge && (
              <div className="flex items-center justify-end mb-2">
                <button
                  onClick={() => setIsExpanded(!isExpanded)}
                  className="text-xs text-muted-foreground hover:text-foreground flex items-center gap-1 transition-colors"
                >
                  {isExpanded ? (
                    <>
                      <ChevronDown className="h-3 w-3" />
                      Collapse
                    </>
                  ) : (
                    <>
                      <ChevronRight className="h-3 w-3" />
                      Expand
                    </>
                  )}
                </button>
              </div>
            )}
            <div className="markdown-body text-sm min-w-0 max-w-full">
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
                      <MemoizedCodeBlock
                        language={match ? match[1] : "text"}
                        code={codeStr}
                      />
                    );
                  },

                  // 标题
                  h1({ children, ...props }) {
                    return <h1 className="text-xl font-semibold mt-4 mb-2" {...props}>{children}</h1>;
                  },
                  h2({ children, ...props }) {
                    return <h2 className="text-lg font-semibold mt-3 mb-2" {...props}>{children}</h2>;
                  },
                  h3({ children, ...props }) {
                    return <h3 className="text-base font-semibold mt-2 mb-1" {...props}>{children}</h3>;
                  },
                  h4({ children, ...props }) {
                    return <h4 className="text-sm font-semibold mt-2 mb-1" {...props}>{children}</h4>;
                  },
                  h5({ children, ...props }) {
                    return <h5 className="text-sm font-medium mt-1 mb-0.5" {...props}>{children}</h5>;
                  },
                  h6({ children, ...props }) {
                    return <h6 className="text-sm font-medium mt-1 mb-0.5" {...props}>{children}</h6>;
                  },

                  // 段落
                  p({ children, ...props }) {
                    return <p className="mb-2 last:mb-0" {...props}>{children}</p>;
                  },

                  // 列表
                  ul({ children, ...props }) {
                    return <ul className="list-disc pl-6 mb-2" {...props}>{children}</ul>;
                  },
                  ol({ children, ...props }) {
                    return <ol className="list-decimal pl-6 mb-2" {...props}>{children}</ol>;
                  },
                  li({ children, ...props }) {
                    return <li className="mb-0.5" {...props}>{children}</li>;
                  },

                  // 引用块
                  blockquote({ children, ...props }) {
                    return (
                      <blockquote className="border-l-2 border-muted-foreground/30 pl-4 py-1 my-2 italic" {...props}>
                        {children}
                      </blockquote>
                    );
                  },

                  // 链接 — 外部链接新窗口打开
                  a({ href, children, ...props }) {
                    return (
                      <a href={href} target="_blank" rel="noopener noreferrer" className="underline" {...props}>
                        {children}
                      </a>
                    );
                  },

                  // 水平分割线
                  hr({ ...props }) {
                    return <hr className="my-4 border-border" {...props} />;
                  },

                  // 图片
                  img({ src, alt, ...props }) {
                    return <img src={src} alt={alt} className="max-w-full rounded-md my-2" loading="lazy" {...props} />;
                  },

                  // 加粗
                  strong({ children, ...props }) {
                    return <strong className="font-semibold" {...props}>{children}</strong>;
                  },

                  // 斜体
                  em({ children, ...props }) {
                    return <em className="italic" {...props}>{children}</em>;
                  },

                  // 删除线 (GFM)
                  del({ children, ...props }) {
                    return <del className="line-through" {...props}>{children}</del>;
                  },

                  // 表格 (GFM)
                  table({ children, ...props }) {
                    return (
                      <div className="overflow-x-auto my-2 max-w-full border rounded-md">
                        <table className="w-full text-sm" {...props}>{children}</table>
                      </div>
                    );
                  },
                  thead({ children, ...props }) {
                    return <thead className="bg-muted/50" {...props}>{children}</thead>;
                  },
                  tbody({ children, ...props }) {
                    return <tbody className="divide-y divide-border" {...props}>{children}</tbody>;
                  },
                  tr({ children, ...props }) {
                    return <tr {...props}>{children}</tr>;
                  },
                  th({ children, ...props }) {
                    return <th className="px-3 py-2 text-left font-medium" {...props}>{children}</th>;
                  },
                  td({ children, ...props }) {
                    return <td className="px-3 py-2" {...props}>{children}</td>;
                  },

                  // 换行
                  br({ ...props }) {
                    return <br {...props} />;
                  },
                }}
              >
                {displayContent || (isStreaming ? "▊" : "")}
              </ReactMarkdown>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// Memoized code block component
interface CodeBlockProps {
  language: string;
  code: string;
}

const MemoizedCodeBlock = memo(function MemoizedCodeBlock({ language, code }: CodeBlockProps) {
  const [isExpanded, setIsExpanded] = useState(true);

  // Check if code block is very large
  const isLarge = code.split("\n").length > 100 || code.length > 5000;
  const displayCode = isLarge && !isExpanded ? code.split("\n").slice(0, 50).join("\n") + "\n\n[... truncated ...]" : code;

  return (
    <div className="my-2">
      {isLarge && (
        <div className="flex items-center justify-between bg-muted/30 px-3 py-1 text-xs rounded-t-md border border-border border-b-0">
          <span className="text-muted-foreground">{language}</span>
          <button
            onClick={() => setIsExpanded(!isExpanded)}
            className="text-muted-foreground hover:text-foreground"
          >
            {isExpanded ? "Collapse" : "Expand"}
          </button>
        </div>
      )}
      <SyntaxHighlighter
        style={oneDark}
        language={language}
        PreTag="div"
        customStyle={{
          margin: 0,
          borderRadius: isLarge ? "0 0 0.5rem 0.5rem" : "0.5rem",
          fontSize: "0.85rem",
          maxHeight: isLarge && isExpanded ? "500px" : undefined,
          overflow: "auto",
        }}
      >
        {displayCode}
      </SyntaxHighlighter>
    </div>
  );
}, (prev, next) => prev.language === next.language && prev.code === next.code);

function truncateContent(content: string): string {
  const lines = content.split("\n");

  if (lines.length <= MAX_CONTENT_LINES && content.length <= MAX_CONTENT_CHARS) {
    return content;
  }

  // Try to truncate at a logical point
  let truncateIndex = -1;
  const targetLines = Math.min(MAX_CONTENT_LINES, Math.floor(lines.length * 0.3));

  // Look for a good section break
  for (let i = targetLines; i < Math.min(targetLines + 50, lines.length); i++) {
    const line = lines[i];
    if (line.match(/^#{1,6}\s/) || line.match(/^---+$/) || line.match(/^\*\*\*+$/)) {
      truncateIndex = i;
      break;
    }
  }

  // If no section break found, just truncate
  const truncatedLines = truncateIndex > 0 ? lines.slice(0, truncateIndex) : lines.slice(0, targetLines);
  const result = truncatedLines.join("\n");

  // Check character length
  if (result.length > MAX_CONTENT_CHARS) {
    const charTruncated = content.slice(0, MAX_CONTENT_CHARS);
    const lastNewline = charTruncated.lastIndexOf("\n");
    const cleanTruncate = lastNewline > MAX_CONTENT_CHARS * 0.7 ? charTruncated.slice(0, lastNewline) : charTruncated;
    return cleanTruncate + "\n\n[... content truncated - click Expand to see full message ...]";
  }

  return result + "\n\n[... content truncated - click Expand to see full message ...]";
}

// Export memoized component
export const AssistantMessage = memo(AssistantMessageComponent, (prev, next) => {
  // Only re-render if content changed or streaming state changed in a meaningful way
  return prev.content === next.content && prev.isStreaming === next.isStreaming;
});
