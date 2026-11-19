import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { icon } from "./icons";
import { t } from "./i18n";

type MusicStatus = {
  tracks: { title: string; artist: string }[];
  track: number;
  playing: boolean;
  volume: number;
  error: string | null;
};

let status: MusicStatus | undefined;
let open = false;
let busy = false;
let failure = "";
const escape = (value: string) => value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);

export function musicTemplate(): string {
  return `<div class="music-player"><button class="icon-button ${status?.playing ? "is-active" : ""}" id="music-button" title="${t("music")}" aria-label="${t("music")}" aria-expanded="${open}" aria-controls="music-panel">${icon("music")}</button><section class="music-panel" id="music-panel" aria-label="${t("music")}" ${open ? "" : "hidden"}></section></div>`;
}

function refresh() {
  const panel = document.querySelector<HTMLElement>("#music-panel");
  const button = document.querySelector("#music-button");
  if (!panel || !button) return;
  panel.hidden = !open;
  button.setAttribute("aria-expanded", String(open));
  button.classList.toggle("is-active", !!status?.playing);
  if (!open) return;
  const track = status?.tracks[status.track];
  const error = failure || status?.error;
  panel.innerHTML = `<strong>${t("music")}</strong>
    <div class="music-track">${track ? escape(track.title) : t("musicLoading")}</div>
    <div class="music-artist">${track ? escape(track.artist) : ""}</div>
    <div class="music-controls"><button class="icon-button" data-music-action="toggle" aria-label="${t(status?.playing ? "musicStop" : "musicPlay")}" title="${t(status?.playing ? "musicStop" : "musicPlay")}" ${!status || busy ? "disabled" : ""}>${icon(status?.playing ? "stop" : "play")}</button></div>
    <label class="music-volume">${t("musicVolume")}<input id="music-volume" type="range" min="0" max="100" step="1" value="${Math.round((status?.volume ?? 0.4) * 100)}" ${!status || busy ? "disabled" : ""}/></label>
    <p class="music-hint">${t("musicHint")}</p>
    ${error ? `<p class="music-error" role="alert">${escape(t("musicFailed", { error }))}</p>` : ""}`;
  panel.querySelectorAll<HTMLButtonElement>("[data-music-action]").forEach((control) => {
    control.addEventListener("click", () => { void controlMusic(control.dataset.musicAction!); });
  });
  // Commit on change, rather than sending IPC and writing preferences per pixel.
  panel.querySelector<HTMLInputElement>("#music-volume")?.addEventListener("change", (event) => {
    void controlMusic("volume", Number((event.target as HTMLInputElement).value) / 100);
  });
}

async function controlMusic(action: string, value?: number) {
  if (busy) return;
  busy = true;
  failure = "";
  refresh();
  try { status = await invoke<MusicStatus>("music_control", { action, value }); }
  catch (error) { failure = String(error); }
  finally { busy = false; refresh(); }
}

export function bindMusic() {
  document.querySelector("#music-button")?.addEventListener("click", () => {
    open = !open;
    refresh();
    if (open && !status) void controlMusic("status");
  });
  refresh();
}

export async function initializeMusic(): Promise<() => void> {
  const unlisten = await listen<MusicStatus>("music-changed", ({ payload }) => {
    status = payload;
    refresh();
  });
  const closeOutside = (event: PointerEvent) => {
    if (!(event.target as Element).closest(".music-player")) { open = false; refresh(); }
  };
  const closeEscape = (event: KeyboardEvent) => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      open = false;
      refresh();
      document.querySelector<HTMLButtonElement>("#music-button")?.focus();
    }
  };
  document.addEventListener("pointerdown", closeOutside);
  document.addEventListener("keydown", closeEscape);
  await controlMusic("status");
  return () => {
    unlisten();
    document.removeEventListener("pointerdown", closeOutside);
    document.removeEventListener("keydown", closeEscape);
  };
}
