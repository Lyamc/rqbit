import { useState } from "react";
import { CgMathPlus } from "react-icons/cg";
import { Button } from "./Button";
import { AddModal } from "../modal/AddModal";

export const AddButton = ({ className }: { className?: string }) => {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button
        onClick={() => setOpen(true)}
        className={`group ${className ?? ""}`}
      >
        <CgMathPlus className="text-blue-500 group-hover:text-white dark:text-white" />
        <div>Add</div>
      </Button>
      {open && <AddModal isOpen={open} onClose={() => setOpen(false)} />}
    </>
  );
};
