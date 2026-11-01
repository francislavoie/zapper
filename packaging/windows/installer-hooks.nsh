; Tauri's NSIS installer hooks (bundle.windows.nsis.installerHooks), used by
; the release's Windows build only: packaging/windows/installers.conf.json.
;
; The NSIS installer and the MSI are bundled from one build with the same
; resources, so both carry install-kind = msi. POSTINSTALL rewrites it to nsis
; after the files are in place, which is how the updater tells the two apart
; and downloads the right installer (src-tauri/src/updates/install.rs).
;
; The recording service, when Settings > Recording installed it, is a
; scheduled task, zapper-server-<user> (src-tauri/src/recording/service.rs,
; task_name), running zapper-server.exe from this folder. A file in use cannot
; be replaced or removed, so zapper-server.exe is stopped before files are
; written or deleted, and the task, if there is one, started again once the
; new ones are in place. This setup is per-user, so taskkill and schtasks
; reach only this user's.
;
; Uninstalling removes the task too, and zapper-server's log, but only when it
; is an uninstall. The uninstaller also runs when a newer version is
; installed over this one, and the task must survive that. Tauri's template
; (tauri-bundler 2.10) runs it in two ways then:
;
;   - with /UPDATE, which sets $UpdateMode, when the setup was itself run
;     with /UPDATE (it then skips the uninstall altogether);
;   - from the setup's "uninstall before installing" choice, as
;     `$INSTDIR\uninstall.exe _?=$INSTDIR`: run in place, from the install
;     folder.
;
; An uninstall from Settings > Apps (or uninstall.exe started by hand, or
; with /S) never runs in place: NSIS copies the uninstaller to a temporary
; folder and runs that copy. So "running from $INSTDIR, or $UpdateMode" means
; another installer is replacing this one; anything else is a real uninstall.
; Recordings, the service's settings and its list of recordings stay, like
; the rest of zapper's data.

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
  ${If} $UpdateMode <> 1
  ${AndIf} $EXEDIR != $INSTDIR
    ReadEnvStr $0 USERNAME
    nsExec::Exec '"$SYSDIR\schtasks.exe" /End /TN "zapper-server-$0"'
    Pop $0
    ReadEnvStr $0 USERNAME
    nsExec::Exec '"$SYSDIR\schtasks.exe" /Delete /TN "zapper-server-$0" /F'
    Pop $0
    !insertmacro ZAPPER_STOP_SERVER
    Delete "$LOCALAPPDATA\zapper\zapper-server.log"
    Delete "$LOCALAPPDATA\zapper\zapper-server.old.log"
  ${Else}
    !insertmacro ZAPPER_STOP_SERVER
  ${EndIf}
!macroend
