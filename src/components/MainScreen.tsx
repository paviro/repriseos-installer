import type React from "react";
import { useHeadingFocus } from "../hooks/useHeadingFocus";
import { ChoiceButton } from "./ui/ChoiceButton";
import { InstallerScreen } from "./ui/InstallerScreen";

interface IMainScreenProps {
  onInstall: () => void;
  onUninstall: () => void;
  onManageExtensions: () => void;
  installDisabled?: boolean;
  updates: React.ReactNode;
}

const MainScreen: React.FC<IMainScreenProps> = ({
  onInstall,
  onUninstall,
  onManageExtensions,
  installDisabled,
  updates,
}) => {
  const { headingRef } = useHeadingFocus();

  return (
    <InstallerScreen.Root aria-labelledby="main-title">
      <InstallerScreen.Title id="main-title" ref={headingRef} tabIndex={-1}>
        RepriseOS
      </InstallerScreen.Title>
      <InstallerScreen.Actions className="mx-auto mt-9 max-w-100 flex-col gap-3">
        <ChoiceButton
          title="Install/Update RepriseOS"
          description="Install or update RepriseOS on your iPod."
          onClick={onInstall}
          disabled={installDisabled}
        />
        <ChoiceButton
          title="Manage extensions"
          description="Add, update or remove extensions on a connected iPod."
          onClick={onManageExtensions}
          disabled={installDisabled}
        />
        <ChoiceButton
          title="Uninstall RepriseOS"
          description="Restore Apple’s bootloader and remove RepriseOS from your iPod."
          onClick={onUninstall}
          disabled={installDisabled}
        />
      </InstallerScreen.Actions>
      {updates}
    </InstallerScreen.Root>
  );
};

export { MainScreen };
