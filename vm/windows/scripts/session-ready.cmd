@echo off
rem Written at every logon by the `sunlit-e2e-session-ready` task.
rem
rem Its existence is what tells the orchestrator that a desktop session exists,
rem which is a different question from "does SSH answer": SSH comes up while
rem the shell is still starting, and a job fired in that window fails
rem confusingly.
setlocal
set ROOT=C:\sunlit-e2e
if not exist "%ROOT%\results" mkdir "%ROOT%\results"
> "%ROOT%\ready" echo %DATE% %TIME%
exit /b 0
