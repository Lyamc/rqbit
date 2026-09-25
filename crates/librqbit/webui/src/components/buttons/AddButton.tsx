import { forwardRef, useState } from "react";
import { CgFolder, CgLink, CgMathPlus, CgSoftwareUpload } from "react-icons/cg";
import { Button } from "./Button";
import { AddModal, AddModalTab } from "../modal/AddModal";

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
          isOpen
          onClose={() => setOpenTab(null)}
          initialTab={openTab === "default" ? undefined : openTab}
        />
      )}
    </>
  );
};
