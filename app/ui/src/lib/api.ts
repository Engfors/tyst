// Typed wrappers around the app's commands and events (app/src-tauri/src/commands.rs, state.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Channel = "me" | "others";
export type Lang = "sv" | "en";
export type LanguageMode = "auto" | "sv" | "en";
export type Phase = "idle" | "starting" | "recording" | "paused" | "naming";

export interface Segment {
  id: number;
  channel: Channel;
  lang: Lang;
  start_ms: number;
  text: string;
}

export interface Partial {
  channel: Channel;
  segment_id: number;
  text: string;
}

export interface Naming {
  placeholder: string;
  path_preview: string;
  seconds: number;
}

export interface Labels {
  me: string;
  others: string;
}

export interface Snapshot {
  phase: Phase;
  elapsed_ms: number;
  language: LanguageMode;
  labels: Labels;
  compact: boolean;
  segments: Segment[];
  partials: Partial[];
  naming: Naming | null;
  warning: string | null;
  channels: Channel[];
}

export type MeetingEvent =
  | ({ type: "state" } & Snapshot)
  | ({ type: "partial" } & Partial)
  | ({ type: "final" } & Segment)
  | { type: "dropped"; channel: Channel; segment_id: number }
  | { type: "level"; channel: Channel; rms: number }
  | { type: "warning"; message: string }
  | { type: "saved"; path: string };

export interface WindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type DictationPhase =
  | "idle"
  | "starting"
  | "listening"
  | "finishing"
  | "preview"
  | "redecoding"
  | "pasting"
  | "pasted"
  | "message";

export type Trigger = "hybrid" | "toggle" | "hold";
export type PasteMode = "preview" | "direct";

export interface PillState {
  phase: DictationPhase;
  text: string;
  partial: string;
  language: LanguageMode;
  paste_mode: PasteMode;
  message: string | null;
  terminal: boolean;
  platform: string;
  languages: LanguageMode[];
}

export type PillEvent =
  | ({ type: "state" } & PillState)
  | { type: "text"; text: string; partial: string }
  | { type: "level"; rms: number };

export interface DictationSettings {
  enabled: boolean;
  trigger: Trigger;
  paste_mode: PasteMode;
  restore_clipboard: boolean;
  terminal_classes: string[];
  language: LanguageMode;
  last_lang: Lang;
  shortcut: string;
  meeting_shortcut: string;
  keyboard_token: string | null;
}

export interface DictationInfo {
  shortcuts: { bound: [string, string][]; error: string | null; pending: boolean };
  keyboard_granted: boolean;
  english_model: boolean;
}

export interface Config {
  onboarded: boolean;
  transcripts_dir: string | null;
  models_dir: string | null;
  language: LanguageMode;
  labels: Labels;
  launch_at_login: boolean;
  threads: number;
  meetings: {
    system_audio: boolean;
    show_window_on_start: boolean;
    compact: boolean;
    name_prompt_seconds: number;
    window: Record<string, WindowGeometry>;
  };
  dictation: DictationSettings;
}

export interface ConfigView {
  config: Config;
  config_dir: string;
  default_transcripts_dir: string;
  default_models_dir: string;
  system_audio_supported: boolean;
  version: string;
  platform: string;
}

export interface ModelRow {
  id: string;
  kind: string;
  version: string;
  license: string;
  size_mb: number;
  status: "installed" | "missing" | "partial" | "broken";
  optional: boolean;
}

export type ModelsEvent =
  | { type: "progress"; file: string; done: number; total: number; note: string }
  | { type: "done" }
  | { type: "failed"; message: string };

export interface Vocabulary {
  terms: string[];
  replacements: { from: string; to: string }[];
}

export const api = {
  appState: () => invoke<Snapshot>("app_state"),
  start: () => invoke<void>("meeting_start"),
  stop: () => invoke<void>("meeting_stop"),
  togglePause: () => invoke<void>("meeting_toggle_pause"),
  setLanguage: (language: LanguageMode) => invoke<void>("meeting_set_language", { language }),
  preview: (title: string | null) => invoke<string | null>("meeting_preview", { title }),
  save: (title: string | null) => invoke<string>("meeting_save", { title }),
  hide: () => invoke<void>("meeting_window_hide"),
  compact: (compact: boolean) => invoke<void>("meeting_window_compact", { compact }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  openTranscripts: () => invoke<void>("open_transcripts_folder"),
  configGet: () => invoke<ConfigView>("config_get"),
  configSet: (config: Config) => invoke<void>("config_set", { config }),
  pickFolder: (current: string | null) => invoke<string | null>("pick_folder", { current }),
  modelsStatus: () => invoke<ModelRow[]>("models_status"),
  modelsFetch: (ids: string[]) => invoke<void>("models_fetch", { ids }),
  modelsCancel: () => invoke<void>("models_cancel"),
  modelsVerify: (full: boolean) => invoke<string[]>("models_verify", { full }),
  modelsInstalled: () => invoke<boolean>("models_installed"),
  vocabularyGet: () => invoke<Vocabulary>("vocabulary_get"),
  vocabularySet: (vocabulary: Vocabulary) => invoke<void>("vocabulary_set", { vocabulary }),
  vocabularyImport: () => invoke<Vocabulary | null>("vocabulary_import"),
  vocabularyExport: () => invoke<string | null>("vocabulary_export"),
  audioTest: (channel: Channel, seconds = 4) =>
    invoke<{ peak: number; text: string }>("audio_test", { channel, seconds }),
  onboardingFinish: () => invoke<void>("onboarding_finish"),
  showSettings: (tab: string | null) => invoke<void>("show_settings", { tab }),
  installWindowRule: () => invoke<void>("install_window_rule"),
  dictationState: () => invoke<PillState | null>("dictation_state"),
  dictationToggle: () => invoke<void>("dictation_toggle"),
  dictationStop: () => invoke<void>("dictation_stop"),
  dictationCancel: () => invoke<void>("dictation_cancel"),
  dictationPaste: (text: string | null) => invoke<void>("dictation_paste", { text }),
  dictationEdit: (text: string) => invoke<void>("dictation_edit", { text }),
  dictationCopy: (text: string) => invoke<void>("dictation_copy", { text }),
  dictationDiscard: () => invoke<void>("dictation_discard"),
  dictationCycleLanguage: () => invoke<void>("dictation_cycle_language"),
  dictationInfo: () => invoke<DictationInfo>("dictation_info"),
  dictationSetupShortcuts: () => invoke<void>("dictation_setup_shortcuts"),
  dictationSetupKeyboard: () => invoke<void>("dictation_setup_keyboard"),
};

export function onDictation(cb: (e: PillEvent) => void): Promise<UnlistenFn> {
  return listen<PillEvent>("tyst://dictation", (e) => cb(e.payload));
}

export function onMeeting(cb: (e: MeetingEvent) => void): Promise<UnlistenFn> {
  return listen<MeetingEvent>("tyst://meeting", (e) => cb(e.payload));
}

export function onModels(cb: (e: ModelsEvent) => void): Promise<UnlistenFn> {
  return listen<ModelsEvent>("tyst://models", (e) => cb(e.payload));
}

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export function formatElapsed(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = String(m).padStart(h ? 2 : 1, "0");
  return `${h ? `${h}:` : ""}${mm}:${String(sec).padStart(2, "0")}`;
}
