import { Bot, ChevronDown, ChevronRight, X } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Light as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
// Only register languages actually used in the app to avoid bundling all 200+
import js from "react-syntax-highlighter/dist/esm/languages/prism/javascript";
import ts from "react-syntax-highlighter/dist/esm/languages/prism/typescript";
import tsx from "react-syntax-highlighter/dist/esm/languages/prism/tsx";
import jsx from "react-syntax-highlighter/dist/esm/languages/prism/jsx";
import python from "react-syntax-highlighter/dist/esm/languages/prism/python";
import rust from "react-syntax-highlighter/dist/esm/languages/prism/rust";
import bash from "react-syntax-highlighter/dist/esm/languages/prism/bash";
import shell from "react-syntax-highlighter/dist/esm/languages/prism/shell-session";
import json from "react-syntax-highlighter/dist/esm/languages/prism/json";
import css from "react-syntax-highlighter/dist/esm/languages/prism/css";
import scss from "react-syntax-highlighter/dist/esm/languages/prism/scss";
import sql from "react-syntax-highlighter/dist/esm/languages/prism/sql";
import markdown from "react-syntax-highlighter/dist/esm/languages/prism/markdown";
import yaml from "react-syntax-highlighter/dist/esm/languages/prism/yaml";
import toml from "react-syntax-highlighter/dist/esm/languages/prism/toml";
import markup from "react-syntax-highlighter/dist/esm/languages/prism/markup";
import http from "react-syntax-highlighter/dist/esm/languages/prism/http";

SyntaxHighlighter.registerLanguage("javascript", js);
SyntaxHighlighter.registerLanguage("js", js);
SyntaxHighlighter.registerLanguage("typescript", ts);
SyntaxHighlighter.registerLanguage("ts", ts);
SyntaxHighlighter.registerLanguage("tsx", tsx);
SyntaxHighlighter.registerLanguage("jsx", jsx);
SyntaxHighlighter.registerLanguage("python", python);
SyntaxHighlighter.registerLanguage("py", python);
SyntaxHighlighter.registerLanguage("rust", rust);
SyntaxHighlighter.registerLanguage("rs", rust);
SyntaxHighlighter.registerLanguage("bash", bash);
SyntaxHighlighter.registerLanguage("sh", shell);
SyntaxHighlighter.registerLanguage("shell", shell);
SyntaxHighlighter.registerLanguage("json", json);
SyntaxHighlighter.registerLanguage("css", css);
SyntaxHighlighter.registerLanguage("scss", scss);
SyntaxHighlighter.registerLanguage("sql", sql);
SyntaxHighlighter.registerLanguage("markdown", markdown);
SyntaxHighlighter.registerLanguage("md", markdown);
SyntaxHighlighter.registerLanguage("yaml", yaml);
SyntaxHighlighter.registerLanguage("yml", yaml);
SyntaxHighlighter.registerLanguage("toml", toml);
SyntaxHighlighter.registerLanguage("xml", markup);
SyntaxHighlighter.registerLanguage("html", markup);
SyntaxHighlighter.registerLanguage("http", http);
import { memo, useState, useMemo, useRef, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

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

                  // 图片 — 解析相对路径并加载本地图片
                  img({ src, alt }) {
                    return (
                      <LocalImage
                        src={src}
                        alt={alt}
                        className="max-w-full rounded-md my-2"
                      />
                    );
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

/**
 * Resolves relative image paths to displayable URLs.
 * Uses a Rust command to read the local file and return a base64 data URL,
 * converted to a blob URL for browser-native caching.
 * Click to open a full-screen image viewer modal with zoom + pan.
 */
interface LocalImageProps {
  src?: string;
  alt?: string;
  className?: string;
}

function LocalImage({ src, alt, className }: LocalImageProps) {
  const imgRef = useRef<HTMLImageElement>(null);
  const blobUrlRef = useRef<string | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  const modalBlobRef = useRef<string | null>(null);

  useEffect(() => {
    const img = imgRef.current;
    if (!img || !src) return;

    // If modal is open with the current blob URL, transfer ownership to modal
    // so the thumbnail can get a fresh blob URL without breaking the modal.
    const currentBlob = blobUrlRef.current;
    if (modalOpen && modalBlobRef.current === currentBlob) {
      blobUrlRef.current = null;
    } else {
      blobUrlRef.current = null;
      if (currentBlob) URL.revokeObjectURL(currentBlob);
    }

    // Pass through data URLs, http(s), blob URLs as-is
    if (/^(https?:|data:|blob:)/.test(src)) {
      img.src = src;
      return () => {};
    }

    let cancelled = false;

    invoke<string>("read_image_file", { imagePath: src.replace(/\\/g, "/") })
      .then((dataUrl) => {
        if (cancelled) return;
        const byteString = atob(dataUrl.split(",")[1]);
        const mimeMatch = dataUrl.match(/data:([^;]+)/);
        const mime = mimeMatch ? mimeMatch[1] : "image/png";
        const ab = new ArrayBuffer(byteString.length);
        const ia = new Uint8Array(ab);
        for (let i = 0; i < byteString.length; i++) {
          ia[i] = byteString.charCodeAt(i);
        }
        const blob = new Blob([ab], { type: mime });
        const url = URL.createObjectURL(blob);
        if (!cancelled) {
          blobUrlRef.current = url;
          img.src = url;
        } else {
          URL.revokeObjectURL(url);
        }
      })
      .catch(() => {
        if (!cancelled) img.src = "";
      });

    return () => { cancelled = true; };
  }, [src, modalOpen]);

  const handleClick = () => {
    const blobUrl = blobUrlRef.current;
    if (blobUrl) {
      modalBlobRef.current = blobUrl;
      blobUrlRef.current = null;
      setModalOpen(true);
    }
  };

  const handleCloseModal = () => {
    setModalOpen(false);
    if (modalBlobRef.current && modalBlobRef.current !== blobUrlRef.current) {
      URL.revokeObjectURL(modalBlobRef.current);
    }
    modalBlobRef.current = null;
  };

  return (
    <>
      <img
        ref={imgRef}
        alt={alt}
        className={className}
        loading="lazy"
        onClick={handleClick}
        style={{ cursor: "zoom-in" }}
      />
      {modalOpen && modalBlobRef.current && (
        <ImageModal src={modalBlobRef.current} alt={alt} onClose={handleCloseModal} />
      )}
    </>
  );
}

/* ------------------------------------------------------------------ */
/*  ImageModal — full-screen viewer with wheel-zoom + drag-pan         */
/* ------------------------------------------------------------------ */

function ImageModal({ src, alt, onClose }: { src: string; alt?: string; onClose: () => void }) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);

  // Transform state stored in refs to avoid re-rendering on every frame
  const transformRef = useRef({ x: 0, y: 0, scale: 1 });

  // Drag tracking
  const dragRef = useRef({ isDragging: false, startX: 0, startY: 0, moved: false });

  // Pinch tracking (touch)
  const pinchRef = useRef({ isPinching: false, startDist: 0, startScale: 1 });

  // Apply CSS transform to the <img> directly (no re-render)
  const applyTransform = () => {
    const el = imgRef.current;
    if (!el) return;
    const { x, y, scale } = transformRef.current;
    el.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
  };

  // Clamp scale to [MIN, MAX]
  const MIN_SCALE = 0.1;
  const MAX_SCALE = 10;
  const clampScale = (s: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));

  /* ---- Wheel zoom ---- */
  const handleWheel = (e: WheelEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const t = transformRef.current;
    const delta = e.deltaY > 0 ? 0.9 : 1.1;
    t.scale = clampScale(t.scale * delta);
    applyTransform();
  };

  /* ---- Mouse drag ---- */
  const onMouseDown = (e: React.MouseEvent) => {
    // Left button only
    if (e.button !== 0) return;
    e.preventDefault();
    const t = transformRef.current;
    dragRef.current = { isDragging: true, startX: e.clientX - t.x, startY: e.clientY - t.y, moved: false };
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const d = dragRef.current;
    if (!d.isDragging) return;
    d.moved = true;
    const t = transformRef.current;
    t.x = e.clientX - d.startX;
    t.y = e.clientY - d.startY;
    applyTransform();
  };

  const onMouseUp = () => {
    dragRef.current.isDragging = false;
  };

  /* ---- Touch: single-finger pan, two-finger pinch ---- */
  const getTouchDist = (touches: React.TouchList) => {
    if (touches.length < 2) return 0;
    const dx = touches[0].clientX - touches[1].clientX;
    const dy = touches[0].clientY - touches[1].clientY;
    return Math.hypot(dx, dy);
  };

  const onTouchStart = (e: React.TouchEvent) => {
    if (e.touches.length === 2) {
      const d = getTouchDist(e.touches);
      pinchRef.current = { isPinching: true, startDist: d, startScale: transformRef.current.scale };
    } else if (e.touches.length === 1) {
      const t = transformRef.current;
      dragRef.current = { isDragging: true, startX: e.touches[0].clientX - t.x, startY: e.touches[0].clientY - t.y, moved: false };
    }
  };

  const onTouchMove = (e: React.TouchEvent) => {
    if (e.touches.length === 2 && pinchRef.current.isPinching) {
      e.preventDefault();
      const d = getTouchDist(e.touches);
      const ratio = d / pinchRef.current.startDist;
      transformRef.current.scale = clampScale(pinchRef.current.startScale * ratio);
      applyTransform();
    } else if (e.touches.length === 1) {
      const d = dragRef.current;
      if (!d.isDragging) return;
      d.moved = true;
      const t = transformRef.current;
      t.x = e.touches[0].clientX - d.startX;
      t.y = e.touches[0].clientY - d.startY;
      applyTransform();
    }
  };

  const onTouchEnd = (e: React.TouchEvent) => {
    if (e.touches.length < 2) pinchRef.current.isPinching = false;
    if (e.touches.length === 0) dragRef.current.isDragging = false;
  };

  /* ---- Keyboard ---- */
  useEffect(() => {
    const el = overlayRef.current;
    if (!el) return;

    // Wheel listener (must be non-passive to call preventDefault)
    el.addEventListener("wheel", handleWheel, { passive: false });

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      el.removeEventListener("wheel", handleWheel);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [onClose]);

  // Click on overlay background → close (but not on image itself or close button)
  const handleOverlayClick = (e: React.MouseEvent) => {
    // If user was dragging the image, don't close
    if (dragRef.current.moved) {
      dragRef.current.moved = false;
      return;
    }
    if (e.target === overlayRef.current) onClose();
  };

  return (
    <div
      ref={overlayRef}
      className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/80 overflow-hidden"
      onMouseDown={handleOverlayClick}
    >
      {/* Close button */}
      <button
        onClick={onClose}
        className="absolute top-4 right-4 z-10 rounded-full bg-black/60 p-2 text-white/80 hover:bg-black/80 hover:text-white transition-colors"
        aria-label="Close"
      >
        <X className="h-5 w-5" />
      </button>

      {/* Zoom hint */}
      <div className="absolute bottom-4 left-1/2 -translate-x-1/2 z-10 text-xs text-white/50 pointer-events-none select-none">
        Scroll to zoom · Drag to pan · Esc to close
      </div>

      {/* Image — handles drag + pinch */}
      <img
        ref={imgRef}
        src={src}
        alt={alt}
        className="object-contain select-none"
        style={{
          transform: "translate(0px, 0px) scale(1)",
          willChange: "transform",
          maxWidth: "92vw",
          maxHeight: "92vh",
        }}
        draggable={false}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
        onMouseLeave={onMouseUp}
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
      />
    </div>
  );
}

// Export memoized component
export const AssistantMessage = memo(AssistantMessageComponent, (prev, next) => {
  // Only re-render if content changed or streaming state changed in a meaningful way
  return prev.content === next.content && prev.isStreaming === next.isStreaming;
});
