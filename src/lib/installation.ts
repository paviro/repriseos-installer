import { backendAvailable, createChannel, invoke } from "./backend.ts";
import { errorDetail } from "./errors.ts";
import type { FirmwareInfo } from "./firmware.ts";
import { appendSessionLog, getSessionLog } from "./sessionLog.ts";

export interface InstallEvent {
  stage: string;
  detail: string;
  completed: number | null;
  total: number | null;
  /** Share of the whole operation done, 0..1. */
  progress: number;
  cancellable: boolean;
  diagnostic?: boolean;
}

/** Why an install or uninstall stopped, as the backend reports it. */
export interface InstallFailure {
  /** Full text for the log, with the stage and backup path. */
  detail: string;
  /** The error alone. */
  error: string;
  stage: string | null;
  /** The iPod stopped answering over USB. */
  device_stopped: boolean;
  /** Share of the Apple firmware decryption kept for the next attempt. */
  kept_percent: number | null;
  /** The backend stopped because the user cancelled. */
  cancelled: boolean;
}

const labels: Record<string, string> = {
  prepare: "Preparing installation",
  storage: "Checking storage",
  checkBootloader: "Checking installed bootloader",
  backup: "Backing up boot firmware",
  decrypt: "Preparing Apple firmware",
  assemble: "Preparing RepriseOS",
  uploadLoader: "Uploading companion loader",
  uploadOsos: "Uploading RepriseOS",
  removeFiles: "Removing RepriseOS files",
  bootloader: "Preparing bootloader installer",
  restart: "Restarting iPod",
};

type Operation = "install" | "uninstall";
type Outcome = "installer_sent" | "files_updated" | "uninstaller_sent" | "files_removed";

interface InstallationState {
  operation: Operation;
  /** An uninstall that leaves the RepriseOS files on the iPod. */
  keepFiles: boolean;
  running: boolean;
  startedAt: number;
  finishedAt: number | null;
  event: InstallEvent;
  failure: InstallFailure | null;
  complete: boolean;
  cancelling: boolean;
  outcome: Outcome | null;
  /** What an uninstall could not delete from the iPod. */
  leftovers: string[];
}

let state: InstallationState = {
  operation: "install",
  keepFiles: false,
  running: false,
  startedAt: 0,
  finishedAt: null,
  event: {
    stage: "prepare",
    detail: "",
    completed: null,
    total: null,
    progress: 0,
    cancellable: true,
  },
  failure: null,
  complete: false,
  cancelling: false,
  outcome: null,
  leftovers: [],
};
const listeners = new Set<() => void>();
const publish = (next: InstallationState) => {
  state = next;
  for (const listener of listeners) listener();
};
export const getInstallationState = () => state;
export function subscribeToInstallation(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function failureOf(error: unknown, detail: string): InstallFailure {
  if (typeof error === "object" && error !== null && "device_stopped" in error) {
    return error as InstallFailure;
  }
  return {
    detail,
    error: detail,
    stage: null,
    device_stopped: false,
    kept_percent: null,
    cancelled: false,
  };
}

// Stages that only read from the iPod; a failure there leaves it as it was.
const readOnlyStages = new Set([
  "prepare",
  "checkBootloader",
  "storage",
  "backup",
  "decrypt",
  "assemble",
]);

// Stages after the bootloader handoff; the iPod may be installing it.
const handoffStages = new Set(["bootloader", "restart"]);

/** Whether waiting for the iPod and running again can fix the failure: it
 * stopped responding before the bootloader handoff. A cancel, full storage
 * or a refused package need the user instead, and DFU mode must not
 * interrupt a bootloader install. */
export function canRetry(failure: InstallFailure) {
  return (
    !failure.cancelled &&
    failure.device_stopped &&
    (failure.stage === null || !handoffStages.has(failure.stage))
  );
}

/** The failure screen's text: what happened, then what to do next, and
 * whether the screen waits for the iPod to retry. */
export function failureMessage(
  failure: InstallFailure,
  { uninstall = false }: { uninstall?: boolean } = {},
): {
  cause: string;
  next: string | null;
  retry: boolean;
} {
  if (
    (failure.stage === "storage" || failure.stage === "removeFiles") &&
    !failure.cancelled &&
    !failure.device_stopped &&
    /\(code -326, errno \d+\)/.test(failure.error)
  ) {
    return {
      cause: "The iPod’s HFS+ volume was not cleanly unmounted. Nothing on it was changed.",
      next: "Let the iPod boot and appear in Finder. Eject it, leave the cable connected, then enter DFU mode again.",
      retry: false,
    };
  }
  const cause = failure.cancelled
    ? `You cancelled the ${uninstall ? "uninstall" : "installation"}.`
    : failure.device_stopped
      ? "Your iPod stopped responding."
      : /[.!?]$/.test(failure.error)
        ? failure.error
        : `${failure.error}.`;
  // Without a stage the failure may have come after the iPod was written.
  const unchanged = failure.stage !== null && readOnlyStages.has(failure.stage);
  const kept = failure.kept_percent;
  const saved =
    kept === null
      ? ""
      : kept > 0
        ? ` Decryption progress (${kept}%) is saved.`
        : " Decryption progress is saved.";
  const retry = canRetry(failure);
  const handoff = failure.stage !== null && handoffStages.has(failure.stage);
  return {
    cause: cause + (unchanged ? " Nothing on it was changed." : "") + saved,
    next: retry
      ? `Put your iPod back into DFU mode to ${kept !== null ? "continue" : "try again"}.`
      : failure.cancelled || failure.stage === null || unchanged
        ? null
        : handoff
          ? "Leave your iPod connected and wait for it to restart before you disconnect it or try again."
          : "Put your iPod back into DFU mode before you try again.",
    retry,
  };
}

export function installationLabel(event: InstallEvent) {
  if (event.stage === "storage") return labels.storage;
  return event.detail || labels[event.stage];
}

let nextJobId = 0;

const completions: Record<Outcome, string> = {
  files_updated:
    "COMPLETE: disk files verified; bootloader unchanged; restart requested; boot not verified.",
  installer_sent:
    "BOOTLOADER HANDOFF: disk files verified; bootloader installer sent; NOR completion not read back.",
  files_removed:
    "COMPLETE: disk files removed; bootloader unchanged; restart requested; boot not verified.",
  uninstaller_sent:
    "BOOTLOADER HANDOFF: bootloader uninstaller sent; NOR completion not read back.",
};

export function startInstallation(firmware: FirmwareInfo, packageDigest: string) {
  if (state.running) return;
  appendSessionLog("Install/update started; checking device state to select the operation.");
  startJob("install", false, "install", { firmwareSha256: firmware.sha256, packageDigest });
}

/** Restore Apple's bootloader, deleting the RepriseOS files unless `keepFiles`. */
export function startUninstall(packageDigest: string, keepFiles: boolean) {
  if (state.running) return;
  appendSessionLog(keepFiles ? "Uninstall started; keeping the files." : "Uninstall started.");
  startJob("uninstall", keepFiles, "uninstall", { packageDigest, keepFiles });
}

function startJob(
  operation: Operation,
  keepFiles: boolean,
  command: string,
  args: Record<string, unknown>,
) {
  publish({
    operation,
    keepFiles,
    running: true,
    startedAt: Date.now(),
    finishedAt: null,
    event: {
      stage: "prepare",
      detail: operation === "uninstall" ? "Preparing uninstall" : "",
      completed: null,
      total: null,
      progress: 0,
      cancellable: true,
    },
    failure: null,
    complete: false,
    cancelling: false,
    outcome: null,
    leftovers: [],
  });
  const jobId = ++nextJobId;
  let lastDetail = "";
  const run = async () => {
    if (!backendAvailable()) throw new Error("Open the desktop installer to install RepriseOS.");
    const channel = createChannel<InstallEvent>();
    channel.onmessage = (event) => {
      if (!state.running || jobId !== nextJobId) return;
      if (event.diagnostic) {
        appendSessionLog(`${event.stage}: ${event.detail}`);
        return;
      }
      if (event.detail !== lastDetail) {
        appendSessionLog(`${event.stage}: ${event.detail}`);
        lastDetail = event.detail;
      }
      publish({ ...state, event });
    };
    return invoke<{
      backup_path: string;
      outcome: Outcome;
      operation: "install" | "update" | "uninstall";
      leftovers?: string[];
    }>(command, { ...args, onEvent: channel });
  };
  void run()
    .then((result) => {
      const leftovers = result.leftovers ?? [];
      appendSessionLog(
        `${result.operation.toUpperCase()} ${completions[result.outcome]} Backups and logs: ${result.backup_path}`,
      );
      if (leftovers.length) appendSessionLog(`Left on the iPod: ${leftovers.join(", ")}`);
      publish({
        ...state,
        running: false,
        complete: true,
        outcome: result.outcome,
        leftovers,
        finishedAt: Date.now(),
      });
    })
    .catch((error: unknown) => {
      const detail = errorDetail(error);
      appendSessionLog(
        `${operation === "install" ? "Installation" : "Uninstall"} stopped: ${detail}`,
      );
      publish({
        ...state,
        running: false,
        failure: failureOf(error, detail),
        finishedAt: Date.now(),
      });
    });
}

export async function cancelInstallation() {
  if (!state.running || !state.event.cancellable || state.cancelling) return;
  publish({ ...state, cancelling: true });
  try {
    await invoke("cancel_install");
    appendSessionLog("Cancellation requested; finishing the current file operation.");
  } catch (error) {
    appendSessionLog(`Cancellation failed: ${errorDetail(error)}`);
    publish({ ...state, cancelling: false });
  }
}

export async function saveDebugZip(): Promise<string | null> {
  return invoke<string | null>("save_debug_zip", {
    sessionLog: getSessionLog().join("\n"),
    preview: false,
  });
}

export function formatElapsedTime(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = String(seconds % 60).padStart(2, "0");
  return hours
    ? `${hours}:${String(minutes).padStart(2, "0")}:${remainder}`
    : `${minutes}:${remainder}`;
}
