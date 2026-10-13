package vault

import (
	"errors"
	"os"
	"path"

	"golang.org/x/sys/unix"
)

// Resolve the parent directories through os.Root, then pass only basenames to
// renameat2. The open descriptors preserve confinement through symlink changes.
// If the kernel/filesystem lacks support, return its error rather than fall
// back to a rename that could silently overwrite the destination.
func (v *Vault) renameNoReplace(from, to string) error {
	fromDir, err := v.root.Open(path.Dir(from))
	if err != nil {
		return err
	}
	defer func() { _ = fromDir.Close() }()
	toDir, err := v.root.Open(path.Dir(to))
	if err != nil {
		return err
	}
	defer func() { _ = toDir.Close() }()
	err = unix.Renameat2(int(fromDir.Fd()), path.Base(from), int(toDir.Fd()), path.Base(to), unix.RENAME_NOREPLACE)
	if errors.Is(err, os.ErrExist) {
		return ErrExists
	}
	if err != nil {
		return &os.LinkError{Op: "renameat", Old: from, New: to, Err: err}
	}
	return nil
}
