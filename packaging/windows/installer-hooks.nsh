; Tauri's NSIS installer hooks (bundle.windows.nsis.installerHooks), used by
; the release's Windows build only: packaging/windows/installers.conf.json.
;
; The NSIS installer and the MSI are bundled from one build with the same
; resources, so both carry install-kind = msi. POSTINSTALL rewrites it to nsis
; after the files are in place, which is how the updater tells the two apart
; and downloads the right installer (src-tauri/src/updates/install.rs).
;
; The recording service, when Settings > Recording installed it, is a
; scheduled task running zapper-server.exe from this folder, and a file in use
; cannot be replaced or removed. So zapper-server.exe is stopped before the
; files are written or deleted, and the task, if there is one, started again
; once the new ones are in place. Not deleted on uninstall: the uninstaller
; also runs when a newer version is installed over this one, which would lose
; the task. Remove it in Settings > Recording first; left behind, it finds
; nothing to start at sign-in and does nothing.
;
; This installer is per-user, so taskkill reaches only this user's
; zapper-server, and the task's name is this user's
; (src-tauri/src/recording/service.rs, task_name).

!macro ZAPPER_STOP_SERVER
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM zapper-server.exe'
  Pop $0
  ; The process may take a moment to let go of its file.
  Sleep 500
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ZAPPER_STOP_SERVER
!macroend

!macro NSIS_HOOK_POSTINSTALL
  FileOpen $0 "$INSTDIR\install-kind" w
  FileWrite $0 "nsis$\r$\n"
  FileClose $0
  ReadEnvStr $0 USERNAME
  nsExec::Exec '"$SYSDIR\schtasks.exe" /Run /TN "zapper-server-$0"'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro ZAPPER_STOP_SERVER
!macroend
