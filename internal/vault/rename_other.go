//go:build !linux

package vault

// Create and Move share the vault mutex, so the application's own creates
// cannot overwrite a move. On these platforms os.Root does not provide an
// atomic no-replace rename; external writers still require coordination.
func (v *Vault) renameNoReplace(from, to string) error {
	return v.root.Rename(from, to)
}
