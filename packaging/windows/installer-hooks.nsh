; Tauri's NSIS installer hooks (bundle.windows.nsis.installerHooks), used by
; the release's Windows build only: packaging/windows/installers.conf.json.
;
; The NSIS installer and the MSI are bundled from one build with the same
; resources, so both carry install-kind = msi. This rewrites it to nsis after
; the files are in place, which is how the updater tells the two apart and
; downloads the right installer (src-tauri/src/updates/install.rs).
;
; NOT VERIFIED: zapper does not build on Windows yet.

!macro NSIS_HOOK_POSTINSTALL
  FileOpen $0 "$INSTDIR\install-kind" w
  FileWrite $0 "nsis$\r$\n"
  FileClose $0
!macroend
