import { invoke } from "@tauri-apps/api/core";
import type { SessionInfo, MessageData, ConfigInfo, ToolCallInfo } from "./types";

function convertMessageData(msg: any): MessageData {
  if (msg.tool_info) {
    return {
      ...msg,
      tool_info: msg.tool_info as ToolCallInfo,
    };
  }
  return msg;
}

export async function createSession(model: string): Promise<SessionInfo> {
  return invoke("create_session", { model });
}

export async function listSessions(): Promise<SessionInfo[]> {
  return invoke("list_sessions");
}

export async function switchSession(sessionId: string): Promise<MessageData[]> {
  const msgs = await invoke<any[]>("switch_session", { sessionId });
  return msgs.map(convertMessageData);
}

export async function sendMessage(
  sessionId: string,
  content: string
): Promise<void> {
  return invoke("send_message", { sessionId, content });
}

export async function cancel(sessionId: string): Promise<void> {
  return invoke("cancel", { sessionId });
}

export async function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

export async function renameSession(
  sessionId: string,
  title: string
): Promise<void> {
  return invoke("rename_session", { sessionId, title });
}

export async function getMessages(sessionId: string): Promise<MessageData[]> {
  const msgs = await invoke<any[]>("get_messages", { sessionId });
  return msgs.map(convertMessageData);
}

export async function confirmTool(
  sessionId: string,
  toolCallId: string,
  approved: boolean
): Promise<void> {
  return invoke("confirm_tool", { sessionId, toolCallId, approved });
}

export async function getConfig(): Promise<ConfigInfo> {
  return invoke("get_config");
}
