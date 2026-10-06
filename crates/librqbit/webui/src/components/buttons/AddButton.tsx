import { forwardRef, useEffect, useState } from "react";
import { CgFolder, CgLink, CgMathPlus, CgSoftwareUpload } from "react-icons/cg";
import { Button } from "./Button";
import { AddModal, AddModalTab } from "../modal/AddModal";
import { takeAddFromLocation } from "../../helper/addFragment";

const iconClass = "text-blue-500 group-hover:text-white dark:text-white";

const ADD_TABS: {
  tab: AddModalTab;
  label: string;
  Icon: React.ComponentType<{ className?: string }>;
}[] = [
  { tab: "upload", label: "Upload", Icon: CgSoftwareUpload },
  { tab: "urls", label: "URL(s)", Icon: CgLink },
  { tab: "browse", label: "Browse server", Icon: CgFolder },
];

/**
 * The three split Add buttons (Upload / URL(s) / Browse server). Purely
 * presentational so the header can also render an invisible copy of it to
 * measure how much width the expanded form needs.
 */
export const AddButtonGroup = forwardRef<
  HTMLDivElement,
  {
    onOpen: (tab: AddModalTab) => void;
    className?: string;
    ariaHidden?: boolean;
  }
>(({ onOpen, className, ariaHidden }, ref) => (
  <div
    ref={ref}
    className={`flex flex-nowrap items-center gap-1 ${className ?? ""}`}
    aria-hidden={ariaHidden || undefined}
  >
    {ADD_TABS.map(({ tab, label, Icon }) => (
      <Button
        key={tab}
        onClick={() => onOpen(tab)}
        className="group whitespace-nowrap"
      >
        <Icon className={iconClass} />
        <div>{label}</div>
      </Button>
    ))}
  </div>
));
AddButtonGroup.displayName = "AddButtonGroup";

/**
 * Header "Add" control. `expanded` shows the three split buttons, each opening
 * the Add modal on its tab; otherwise a single "Add" button (default URL(s)
 * tab, as before).
 */
export const AddButton = ({
  className,
  expanded = false,
}: {
  className?: string;
  expanded?: boolean;
}) => {
  const [openTab, setOpenTab] = useState<AddModalTab | "default" | null>(null);
  // A link from `#add=` (browser magnet handler): staged, not added yet.
  const [initialPaste, setInitialPaste] = useState<string | undefined>();
  // Bumped to remount the modal with a new link.
  const [modalKey, setModalKey] = useState(0);

  useEffect(() => {
    const open = (link: string | null) => {
      if (!link) return;
      setInitialPaste(link);
      setModalKey((k) => k + 1);
      setOpenTab("urls");
    };
    open(takeAddFromLocation());
    const onHash = () => open(takeAddFromLocation(true));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  return (
    <>
      {expanded ? (
        <AddButtonGroup onOpen={(tab) => setOpenTab(tab)} />
      ) : (
        <Button
          onClick={() => setOpenTab("default")}
          className={`group ${className ?? ""}`}
        >
          <CgMathPlus className={iconClass} />
          <div>Add</div>
        </Button>
      )}
      {openTab !== null && (
        <AddModal
          key={modalKey}
          isOpen
          onClose={() => {
            setOpenTab(null);
            setInitialPaste(undefined);
          }}
          initialTab={openTab === "default" ? undefined : openTab}
          initialPaste={initialPaste}
        />
      )}
    </>
  );
};
