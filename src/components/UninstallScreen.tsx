import React from "react";
import { useHeadingFocus } from "../hooks/useHeadingFocus";
import { errorDetail } from "../lib/errors";
import { chooseLocalPackage, type PackageInfo, preparePackage } from "../lib/package";
import { appendSessionLog } from "../lib/sessionLog";
import { LocalPackagePicker } from "./LocalPackagePicker";
import { Button } from "./ui/Button";
import { InstallerScreen } from "./ui/InstallerScreen";
import { ScreenReveal } from "./ui/ScreenReveal";
import { WarningSymbol } from "./ui/WarningSymbol";

interface IUninstallScreenProps {
  localPackage: PackageInfo | null;
  onSelectPackage: (value: PackageInfo | null) => void;
  keepFiles: boolean;
  onKeepFiles: (value: boolean) => void;
  onContinue: (packageDigest: string) => void;
  onBack: () => void;
}

/** The package carries the bootloader uninstaller, so it is prepared here as the
 * firmware screen does for an install, without an IPSW. */
const UninstallScreen: React.FC<IUninstallScreenProps> = ({
  localPackage,
  onSelectPackage,
  keepFiles,
  onKeepFiles,
  onContinue,
  onBack,
}) => {
  const { headingRef } = useHeadingFocus();
  const [loading, setLoading] = React.useState<"choose" | "prepare" | null>(null);
  const [packageError, setPackageError] = React.useState("");

  const run = async (action: "choose" | "prepare") => {
    setLoading(action);
    setPackageError("");
    try {
      if (action === "choose") {
        const selected = await chooseLocalPackage();
        if (selected) {
          onSelectPackage(selected);
          appendSessionLog(`Local package loaded: ${selected.filename} (${selected.version}).`);
        }
        return;
      }
      const selected = await preparePackage(null, localPackage);
      appendSessionLog(`Package ready: ${selected.version} (${selected.digest}).`);
      onContinue(selected.digest);
    } catch (error) {
      const detail = errorDetail(error);
      setPackageError(detail);
      appendSessionLog(
        `${action === "choose" ? "Package loading" : "Package preparation"} failed: ${detail}`,
      );
    } finally {
      setLoading(null);
    }
  };

  return (
    <InstallerScreen.Root aria-labelledby="uninstall-title">
      <ScreenReveal>
        <div className="mx-auto mb-8 h-28 w-30" aria-hidden="true">
          <WarningSymbol />
        </div>
        <InstallerScreen.Title id="uninstall-title" ref={headingRef} tabIndex={-1}>
          Remove RepriseOS.
        </InstallerScreen.Title>
      </ScreenReveal>
      <ScreenReveal delay={0.08}>
        <InstallerScreen.Description className="max-w-125">
          {keepFiles
            ? "This restores Apple’s bootloader and leaves RepriseOS, its extensions and their settings on your iPod. Your music and Apple’s firmware stay too."
            : "This restores Apple’s bootloader and deletes RepriseOS, its extensions and their settings. Your music and Apple’s firmware stay on your iPod."}
        </InstallerScreen.Description>
        <label className="installer-well mx-auto mt-7 flex max-w-125 cursor-pointer items-start gap-3 rounded-lg p-4 text-left">
          <input
            type="checkbox"
            className="mt-1 accent-[#3e89c5]"
            checked={keepFiles}
            disabled={!!loading}
            onChange={(event) => onKeepFiles(event.target.checked)}
          />
          <span className="text-[13px] leading-5">
            <span className="block font-medium text-ink">Keep the RepriseOS files</span>
            <span className="mt-1 block text-[12px] text-body">
              Only Apple’s bootloader is restored. Your extensions and their settings stay in the
              Reprise folder for a later install.
            </span>
          </span>
        </label>
        <LocalPackagePicker
          localPackage={localPackage}
          loading={loading}
          disabled={!!loading}
          packageError={packageError}
          onChoose={() => run("choose")}
          onAutomatic={() => {
            onSelectPackage(null);
            setPackageError("");
            appendSessionLog("Automatic package download selected.");
          }}
        />
      </ScreenReveal>
      <ScreenReveal delay={0.16}>
        <InstallerScreen.Actions>
          <Button.Root disabled={!!loading} onClick={onBack}>
            <Button.Label>Back</Button.Label>
          </Button.Root>
          <Button.Root disabled={!!loading} onClick={() => run("prepare")}>
            <Button.Label>
              {loading === "prepare" ? (localPackage ? "Preparing…" : "Downloading…") : "Continue"}
            </Button.Label>
          </Button.Root>
        </InstallerScreen.Actions>
      </ScreenReveal>
    </InstallerScreen.Root>
  );
};

export { UninstallScreen };
