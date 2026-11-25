import { useState, useEffect, useCallback } from "react";
import {
  AlarmClock,
  Pause,
  Play,
  Trash2,
  ChevronDown,
  ChevronRight,
  RefreshCw,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  listScheduledTasks,
  getScheduledTask,
  pauseScheduledTask,
  resumeScheduledTask,
  deleteScheduledTask,
} from "@/lib/commands";
import type { ScheduleTaskDto, ScheduleRunDto } from "@/lib/types";

interface SchedulePanelProps {
  onClose: () => void;
}

function formatTime(iso: string | null): string {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    return d.toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

function scheduleLabel(schedule: Record<string, unknown>): string {
  switch (schedule.kind) {
    case "once":
      return `一次性 @ ${formatTime(schedule.at as string)}`;
    case "interval":
      return `每 ${(schedule.every_secs as number)}s`;
    case "cron":
      return `cron: ${schedule.expr}`;
    default:
      return JSON.stringify(schedule);
  }
}

function statusBadge(status: string): string {
  switch (status) {
    case "active":
      return "🟢 运行中";
    case "paused":
      return "⏸️ 已暂停";
    case "completed":
      return "✅ 已完成";
    default:
      return status;
  }
}

export function SchedulePanel({ onClose }: SchedulePanelProps) {
  const [tasks, setTasks] = useState<ScheduleTaskDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [runs, setRuns] = useState<Record<string, ScheduleRunDto[]>>({});

  const refresh = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const data = await listScheduledTasks();
      setTasks(data);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleToggleExpand = async (taskId: string) => {
    if (expandedId === taskId) {
      setExpandedId(null);
      return;
    }
    setExpandedId(taskId);
    try {
      const [, taskRuns] = await getScheduledTask(taskId);
      setRuns((prev) => ({ ...prev, [taskId]: taskRuns }));
    } catch (e) {
      console.error("Failed to load runs:", e);
    }
  };

  const handleAction = async (
    taskId: string,
    action: "pause" | "resume" | "delete"
  ) => {
    try {
      switch (action) {
        case "pause":
          await pauseScheduledTask(taskId);
          break;
        case "resume":
          await resumeScheduledTask(taskId);
          break;
        case "delete":
          await deleteScheduledTask(taskId);
          break;
      }
      await refresh();
      if (expandedId === taskId && action !== "delete") {
        const [, taskRuns] = await getScheduledTask(taskId);
        setRuns((prev) => ({ ...prev, [taskId]: taskRuns }));
      }
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b shrink-0">
        <div className="flex items-center gap-1.5">
          <AlarmClock className="h-4 w-4" />
          <span className="text-sm font-medium">计划任务</span>
          <span className="text-xs text-muted-foreground">
            ({tasks.length})
          </span>
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={refresh}
            title="刷新"
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={onClose}
            title="关闭"
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {/* Content */}
      <ScrollArea className="flex-1 min-h-0">
        <div className="p-2 space-y-1">
          {loading && (
            <p className="text-xs text-muted-foreground text-center py-8">
              加载中...
            </p>
          )}
          {error && (
            <p className="text-xs text-destructive text-center py-4">
              {error}
            </p>
          )}
          {!loading && !error && tasks.length === 0 && (
            <p className="text-xs text-muted-foreground text-center py-8">
              没有计划任务。在对话中让 Agent 创建（如「每天 9 点提醒我」）。
            </p>
          )}
          {tasks.map((task) => (
            <div key={task.id} className="rounded-md border">
              {/* Task row */}
              <div
                className="flex items-center gap-2 px-2 py-1.5 cursor-pointer hover:bg-accent/50"
                onClick={() => handleToggleExpand(task.id)}
              >
                {expandedId === task.id ? (
                  <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                )}
                <div className="flex-1 min-w-0">
                  <div className="text-sm font-medium truncate">
                    {task.title}
                  </div>
                  <div className="text-xs text-muted-foreground truncate">
                    {scheduleLabel(task.schedule)} · {statusBadge(task.status)} ·{" "}
                    下次: {formatTime(task.next_run_at)}
                  </div>
                </div>
                {/* Action buttons */}
                <div
                  className="flex items-center gap-0.5 shrink-0"
                  onClick={(e) => e.stopPropagation()}
                >
                  {task.status === "active" && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6"
                      onClick={() => handleAction(task.id, "pause")}
                      title="暂停"
                    >
                      <Pause className="h-3 w-3" />
                    </Button>
                  )}
                  {task.status === "paused" && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6"
                      onClick={() => handleAction(task.id, "resume")}
                      title="恢复"
                    >
                      <Play className="h-3 w-3" />
                    </Button>
                  )}
                  {task.status !== "deleted" && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6 text-destructive hover:text-destructive"
                      onClick={() => handleAction(task.id, "delete")}
                      title="删除"
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  )}
                </div>
              </div>

              {/* Expanded detail */}
              {expandedId === task.id && (
                <div className="px-3 pb-2 pt-1 border-t text-xs space-y-2">
                  <div>
                    <span className="text-muted-foreground">提示词：</span>
                    <pre className="mt-0.5 whitespace-pre-wrap break-words bg-muted/50 rounded p-1.5 max-h-24 overflow-auto">
                      {task.prompt}
                    </pre>
                  </div>
                  <div className="grid grid-cols-2 gap-x-4 gap-y-0.5 text-muted-foreground">
                    <span>触发次数: {task.run_count}</span>
                    <span>
                      上限: {task.max_runs ?? "不限"}
                    </span>
                    <span>时区: {task.timezone || "本地"}</span>
                    <span>上次: {formatTime(task.last_run_at)}</span>
                    <span className="col-span-2">
                      ID: {task.id.slice(0, 8)}
                    </span>
                  </div>

                  {/* Run history */}
                  {runs[task.id] && runs[task.id].length > 0 && (
                    <div>
                      <span className="text-muted-foreground font-medium">
                        运行历史（最近 {runs[task.id].length} 条）：
                      </span>
                      <div className="mt-1 space-y-0.5 max-h-32 overflow-auto">
                        {runs[task.id].map((run) => (
                          <div
                            key={run.id}
                            className="flex items-center gap-2 text-muted-foreground"
                          >
                            <span
                              className={`inline-block w-2 h-2 rounded-full shrink-0 ${
                                run.status === "delivered"
                                  ? "bg-green-500"
                                  : run.status === "failed"
                                    ? "bg-red-500"
                                    : run.status === "claimed"
                                      ? "bg-yellow-500"
                                      : "bg-gray-400"
                              }`}
                            />
                            <span>{formatTime(run.fired_at)}</span>
                            <span>{run.status}</span>
                            {run.catch_up && (
                              <span className="text-yellow-600">补触发</span>
                            )}
                            {run.error && (
                              <span className="text-red-500 truncate">
                                {run.error}
                              </span>
                            )}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      </ScrollArea>
    </div>
  );
}
