import { RefObject, useRef, useState } from "react";
import { UploadButton } from "./UploadButton";
import { CgFileAdd } from "react-icons/cg";
import { BulkImportModal } from "../modal/BulkImportModal";

export const FileInput = ({ className }: { className?: string }) => {
  const inputRef = useRef<HTMLInputElement>(
    null,
  ) as RefObject<HTMLInputElement>;
  const [file, setFile] = useState<File | null>(null);
  const [bulkFiles, setBulkFiles] = useState<File[] | null>(null);

  const onFileChange = async () => {
    if (!inputRef?.current?.files) {
      return;
    }
    if (inputRef.current.files.length == 1) {
      const file = inputRef.current.files[0];
      setFile(file);
    } else if (inputRef.current.files.length > 1) {
      const files = Array.from(inputRef.current.files);
      // Reset the input so the same multi-select can be chosen again later.
      inputRef.current.value = "";
      setBulkFiles(files);
    }
  };

  const reset = () => {
    if (!inputRef?.current) {
      return;
    }
    inputRef.current.value = "";
    setFile(null);
  };

  const onClick = () => {
    if (!inputRef?.current) {
      return;
    }
    inputRef.current.click();
  };

  return (
    <>
      <input
        type="file"
        ref={inputRef}
        multiple={true}
        accept=".torrent"
        onChange={onFileChange}
        hidden
      />
      <UploadButton
        onClick={onClick}
        data={file}
        resetData={reset}
        className={`group ${className}`}
      >
        <CgFileAdd className="text-blue-500 group-hover:text-white dark:text-white" />
        <div>Upload .torrent File</div>
      </UploadButton>
      {bulkFiles && (
        <BulkImportModal
          isOpen={true}
          initialFiles={bulkFiles}
          onClose={() => setBulkFiles(null)}
        />
      )}
    </>
  );
};
