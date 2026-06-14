export interface SessionInfo {
  id: string;
  title: string;
  model: string;
  status: "idle" | "ready" | "running";
  created_at: string;
  updated_at: string;
}

export interface MessageData {
  id: number;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  tool_name?: string;
  tool_call_id?: string;
  tool_info?: ToolCallInfo;
  created_at: string;
}

export interface ConfigInfo {
  model: string;
  version: string;
  tools_enabled: number;
  tools_total: number;
  skills_enabled: number;
  skills_total: number;
  auto_approve: boolean;
  working_dir: string;
}

export interface ToolCallInfo {
  tool_call_id: string;
  name: string;
  arguments: string;
  status: "running" | "success" | "error" | "awaiting_confirmation";
  output?: string;
  requires_confirm: boolean;
}

export type UiEvent =
  | { type: "TextDelta"; session_id: string; delta: string }
  | {
      type: "ToolCallRequested";
      session_id: string;
      tool_call_id: string;
      name: string;
      arguments: string;
      requires_confirm: boolean;
    }
  | {
      type: "ToolCallResult";
      session_id: string;
      tool_call_id: string;
      content: string;
      is_error: boolean;
    }
  | { type: "TurnComplete"; session_id: string }
  | { type: "Error"; session_id: string; message: string }
  | {
      type: "SkillTriggered";
      session_id: string;
      name: string;
      description: string;
    }
  | { type: "SessionRenamed"; session_id: string; title: string };
