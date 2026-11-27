import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import React from "react";
import { DeviceChecksScreen } from "./components/DeviceChecksScreen";
import { DfuScreen } from "./components/DfuScreen";
import { DisclaimerScreen } from "./components/DisclaimerScreen";
import { ExtensionsScreen } from "./components/extensions/ExtensionsScreen";
import { FirmwareScreen } from "./components/FirmwareScreen";
import { InstallationScreen } from "./components/InstallationScreen";
import { InstallerUpdates } from "./components/InstallerUpdates";
import { MainScreen } from "./components/MainScreen";
import { SuccessScreen } from "./components/SuccessScreen";
import { UninstallScreen } from "./components/UninstallScreen";
import { UsbPermissionsScreen } from "./components/UsbPermissionsScreen";
import { DebugLog } from "./components/ui/DebugLog";
import { ExternalLink } from "./components/ui/ExternalLink";
import { startDeviceChecks } from "./lib/deviceChecks";
import type { FirmwareInfo } from "./lib/firmware";
import { startInstallation, startUninstall } from "./lib/installation";
import type { PackageInfo } from "./lib/package";
import { appendSessionLog } from "./lib/sessionLog";
import { getUpdateState, subscribeToUpdates, updateIsBusy } from "./lib/updates";
import "./App.css";

export type Screen =
  | "main"
  | "disclaimer"
  | "firmware"
  | "uninstall"
  | "permissions"
  | "dfu"
  | "checks"
  | "installation"
  | "success"
  | "extensions";

type Mode = "install" | "uninstall";

interface AppProps {
  initial?: {
    screen: Screen;
    mode?: Mode;
    keepFiles?: boolean;
    firmware: FirmwareInfo | null;
    packageDigest: string;
    localPackage?: PackageInfo;
  };
}

const disclaimerKey = "disclaimerAcknowledged";

const App: React.FC<AppProps> = ({ initial }) => {
  const updater = React.useSyncExternalStore(subscribeToUpdates, getUpdateState);
  const [screen, setScreen] = React.useState<Screen>(initial?.screen ?? "main");
  const [disclaimerAcknowledged, setDisclaimerAcknowledged] = React.useState(() => {
    if (initial) return false;
    try {
      return localStorage.getItem(disclaimerKey) === "true";
    } catch {
      return false;
    }
  });
  const [firmware, setFirmware] = React.useState<FirmwareInfo | null>(initial?.firmware ?? null);
  const [localPackage, setLocalPackage] = React.useState<PackageInfo | null>(
    initial?.localPackage ?? null,
  );
  const [packageDigest, setPackageDigest] = React.useState(initial?.packageDigest ?? "");
  const [mode, setMode] = React.useState<Mode>(initial?.mode ?? "install");
  const [keepFiles, setKeepFiles] = React.useState(initial?.keepFiles ?? false);
  const reducedMotion = useReducedMotion();
  const scrollRef = React.useRef<HTMLElement>(null);

  // Where Back and Change firmware return to: the screen that prepared the package.
  const packageScreen: Screen = mode === "uninstall" ? "uninstall" : "firmware";
  const begin = () => {
    if (!packageDigest) return;
    if (mode === "uninstall") startUninstall(packageDigest, keepFiles);
    else if (firmware) startInstallation(firmware, packageDigest);
    else return;
    setScreen("installation");
  };

  return (
    <main
      ref={scrollRef}
      className="app-surface h-full overflow-x-hidden overflow-y-auto overscroll-none"
    >
      <div
        className={`flex min-h-full flex-col items-center px-6 sm:px-12 ${screen === "dfu" ? "py-8" : "py-14"}`}
      >
        <AnimatePresence mode="wait" onExitComplete={() => scrollRef.current?.scrollTo({ top: 0 })}>
          <motion.div
            key={screen}
            className={
              screen === "extensions"
                ? "mb-auto w-full shrink-0"
                : "my-auto w-full max-w-160 shrink-0"
            }
            initial={{ opacity: 0, y: reducedMotion || screen === "dfu" ? 0 : 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reducedMotion ? 0 : -8 }}
            transition={{ duration: 0.25 }}
          >
            {screen === "main" ? (
              <MainScreen
                updates={<InstallerUpdates />}
                installDisabled={updateIsBusy(updater.status)}
                onManageExtensions={() => {
                  appendSessionLog("Extension management opened.");
                  setScreen("extensions");
                }}
                onInstall={() => {
                  appendSessionLog("New setup started.");
                  setMode("install");
                  setFirmware(null);
                  setLocalPackage(null);
                  setPackageDigest("");
                  setScreen(disclaimerAcknowledged ? "firmware" : "disclaimer");
                }}
                onUninstall={() => {
                  appendSessionLog("Uninstall setup started.");
                  setMode("uninstall");
                  setKeepFiles(false);
                  setLocalPackage(null);
                  setPackageDigest("");
                  setScreen("uninstall");
                }}
              />
            ) : screen === "disclaimer" ? (
              <DisclaimerScreen
                onContinue={() => {
                  setDisclaimerAcknowledged(true);
                  if (!initial) {
                    try {
                      localStorage.setItem(disclaimerKey, "true");
                    } catch {
                      appendSessionLog("Could only remember the warning for this session.");
                    }
                  }
                  appendSessionLog("Installation disclaimer acknowledged.");
                  setScreen("firmware");
                }}
                onQuit={() => setScreen("main")}
              />
            ) : screen === "firmware" ? (
              <FirmwareScreen
                file={firmware}
                localPackage={localPackage}
                onSelectPackage={setLocalPackage}
                onSelect={(file) => {
                  setFirmware(file);
                  if (file) {
                    appendSessionLog(`Firmware validated: ${file.filename} (${file.version}).`);
                  }
                }}
                onContinue={(digest) => {
                  setPackageDigest(digest);
                  setScreen("permissions");
                }}
                onBack={() => setScreen("main")}
              />
            ) : screen === "uninstall" ? (
              <UninstallScreen
                localPackage={localPackage}
                onSelectPackage={setLocalPackage}
                keepFiles={keepFiles}
                onKeepFiles={setKeepFiles}
                onContinue={(digest) => {
                  setPackageDigest(digest);
                  setScreen("permissions");
                }}
                onBack={() => setScreen("main")}
              />
            ) : screen === "permissions" ? (
              <UsbPermissionsScreen
                onContinue={() => {
                  appendSessionLog("Waiting for DFU connection.");
                  setScreen("dfu");
                }}
                onBack={() => setScreen(packageScreen)}
              />
            ) : screen === "dfu" ? (
              <DfuScreen
                onContinue={(device) => {
                  startDeviceChecks(device);
                  setScreen("checks");
                }}
              />
            ) : screen === "checks" ? (
              <DeviceChecksScreen
                onContinue={begin}
                uninstall={mode === "uninstall"}
                onBack={() => setScreen("dfu")}
                onChangeFirmware={() => {
                  setPackageDigest("");
                  setScreen(packageScreen);
                }}
              />
            ) : screen === "installation" ? (
              <InstallationScreen
                onComplete={() => setScreen("success")}
                onRetry={begin}
                onBack={() => setScreen("main")}
              />
            ) : screen === "extensions" ? (
              <ExtensionsScreen onBack={() => setScreen("main")} />
            ) : (
              <SuccessScreen onDone={() => setScreen("main")} />
            )}
          </motion.div>
        </AnimatePresence>
      </div>
      <ExternalLink
        href="https://ko-fi.com/lgreitis"
        className="absolute bottom-4 left-4 rounded text-xs text-muted no-underline hover:underline"
      >
        <span aria-hidden="true" className="text-red-500">
          ♥
        </span>{" "}
        Support on Ko-fi
      </ExternalLink>
      {screen !== "extensions" && <DebugLog />}
      {import.meta.env.DEV && !initial && screen === "main" && !updateIsBusy(updater.status) && (
        <a
          href="?dev-panel"
          className="fixed right-3 bottom-3 rounded border border-black/20 bg-white px-3 py-2 text-xs text-black"
        >
          Dev panel
        </a>
      )}
    </main>
  );
};

export default App;
