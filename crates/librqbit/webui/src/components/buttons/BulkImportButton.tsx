import { useState } from "react";
import { CgSoftwareDownload } from "react-icons/cg";
import { Button } from "./Button";
import { BulkImportModal } from "../modal/BulkImportModal";

export const BulkImportButton = ({ className }: { className?: string }) => {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button
        onClick={() => setOpen(true)}
        className={`group ${className ?? ""}`}
      >
        <CgSoftwareDownload className="text-blue-500 group-hover:text-white dark:text-white" />
        <div>Import many</div>
      </Button>
      {open && (
        <BulkImportModal isOpen={open} onClose={() => setOpen(false)} />
      )}
    </>
  );
};
