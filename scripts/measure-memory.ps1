<#
.SYNOPSIS
  Memory use of Ligma (or the Figma desktop app) and all its child processes.

.DESCRIPTION
  Adds up the private working set, the number Task Manager shows as
  "Memory", over the app's process tree. That includes the WebView2
  (msedgewebview2.exe) browser, GPU and renderer processes Ligma starts.

.EXAMPLE
  ./scripts/measure-memory.ps1                # Ligma
  ./scripts/measure-memory.ps1 -Name Figma    # Figma desktop, to compare
  ./scripts/measure-memory.ps1 -Watch 5       # repeat every 5 seconds
#>
param(
  [string]$Name = "ligma",
  [int]$Watch = 0
)

function Measure-Tree {
  $processes = Get-CimInstance Win32_Process
  $children = @{}
  foreach ($p in $processes) { $children[[int]$p.ParentProcessId] += @($p) }

  # Top of each tree: a matching process whose parent isn't one too
  $matching = $processes | Where-Object { $_.Name -eq "$Name.exe" }
  $ids = @($matching.ProcessId)
  $queue = [System.Collections.Generic.Queue[object]]::new()
  $matching | Where-Object { $ids -notcontains $_.ParentProcessId } | ForEach-Object { $queue.Enqueue($_) }
  if ($queue.Count -eq 0) { Write-Host "No $Name.exe running."; return }

  $tree = while ($queue.Count) {
    $p = $queue.Dequeue()
    $p
    foreach ($child in $children[[int]$p.ProcessId]) { if ($child) { $queue.Enqueue($child) } }
  }

  $private = @{}
  Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
    ForEach-Object { $private[[int]$_.IDProcess] = [long]$_.WorkingSetPrivate }

  $rows = foreach ($p in $tree) {
    # Chromium processes say what they are with --type=renderer, --type=gpu-process, ...
    $type = if ($p.CommandLine -match "--type=([\w-]+)") { $Matches[1] } else { "main" }
    [pscustomobject]@{
      Process = $p.Name
      PID     = $p.ProcessId
      Type    = $type
      MB      = [math]::Round($private[[int]$p.ProcessId] / 1MB, 1)
    }
  }
  $rows | Sort-Object MB -Descending | Format-Table -AutoSize | Out-String | Write-Host
  $total = ($rows | Measure-Object MB -Sum).Sum
  Write-Host ("{0}: {1:N0} MB in {2} processes" -f $Name, $total, @($rows).Count)
}

do {
  Measure-Tree
  if ($Watch -gt 0) { Start-Sleep -Seconds $Watch; Write-Host "" }
} while ($Watch -gt 0)
