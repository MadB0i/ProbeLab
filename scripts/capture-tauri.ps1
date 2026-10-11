# Captures the real Tauri window, not a browser render.
#
# The whole redesign was judged against browser screenshots with a stubbed IPC boundary, which is
# necessary for layout work and useless as proof: it cannot show whether the native window chrome,
# the real WebView2 font rasterisation, or the actual backend read paths work. This drives the built
# executable and captures its window with PrintWindow, so what lands in the report is the application
# a user would launch.
#
# Routes are switched through the DevTools protocol (see scripts/set-route.mjs): the app routes on
# `location.hash`, an already-running process cannot be told to change it any other way, and
# restarting per route would rebuild the window and the backend for every shot.
#
# Usage: capture-tauri.ps1 [-Routes lab,readiness] [-Width 1600] [-Height 900] [-OutDir <dir>]
param(
  [string]$Exe = "D:\Projects\ProbeLab\target\release\probelab-desktop.exe",
  [string]$Routes = "lab,readiness,agents,benchmarks,tasks,comparison,providers,settings,arena,security,replay",
  [int]$Width = 1600,
  [int]$Height = 900,
  [int]$Port = 9333,
  [int]$SettleMs = 1600,
  [string]$OutDir = "C:\Users\rupjy\AppData\Local\Temp\opencode\capture"
)

Add-Type -AssemblyName System.Drawing

if (-not ("Win32Capture" -as [type])) {
  Add-Type @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;

public class Win32Capture {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int X, int Y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);

  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  public const uint PW_RENDERFULLCONTENT = 0x00000002;
}
"@
}

# This script has to see the window in the same units the window is actually created in.
#
# Without this, PowerShell runs as a DPI-unaware process and Windows virtualises every coordinate
# it is given: `GetWindowRect` reports logical pixels while `PrintWindow` paints physical ones, so
# the bitmap sized from the rectangle comes back smaller than the surface and the right-hand column
# of every capture is silently cut away. The layout inside the page is then *also* wrong in the
# capture, which looks like a CSS bug and is not one. Claiming per-monitor awareness here makes
# rectangle, surface and page all speak physical pixels.
if (-not [Win32Capture]::SetProcessDpiAwarenessContext([IntPtr](-4))) {
  [void][Win32Capture]::SetProcessDPIAware()
}

if (-not (Test-Path -LiteralPath $OutDir)) {
  New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
}

$routeNames = @($Routes -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($routeNames.Count -eq 0) { Write-Output "FAILED: no routes requested"; exit 2 }

Get-Process probelab-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

# Loopback-only DevTools, required so a route can be selected in the running window.
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"

$proc = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 7

$proc.Refresh()
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
  Write-Output "FAILED: no main window handle. The process may have exited: $($proc.HasExited)"
  exit 1
}

$setRoute = Join-Path $PSScriptRoot "set-route.mjs"

function Get-Viewport([string]$text) {
  if ($text -match "viewport=(\d+)x(\d+)") {
    return @{ Width = [int]$Matches[1]; Height = [int]$Matches[2] }
  }
  return $null
}

# PW_RENDERFULLCONTENT is required: without it WebView2 captures as a blank/black surface.
[void][Win32Capture]::ShowWindow($hwnd, 9)   # SW_RESTORE
[void][Win32Capture]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 800

$failures = 0

# Measure, size once, verify.
#
# Three units are in play and confusing them is how a capture ends up silently cropped: the window
# rectangle is physical pixels, the page viewport is CSS pixels, and the two are related by the
# window's DPI. The border and title bar are measured rather than assumed, and the result is
# confirmed against the viewport the page actually laid out -- if it is off by a pixel after the
# resize, the window is corrected once more rather than shipping a capture of the wrong size.
$dpi = [Win32Capture]::GetDpiForWindow($hwnd)
$scale = $dpi / 96.0

$probe = & node $setRoute $routeNames[0] $Port $SettleMs 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $failures++ }
Write-Output $probe.Trim()

$viewport = Get-Viewport $probe
if (-not $viewport) {
  Write-Output "FAILED: could not read the viewport from the running window"
  Get-Process probelab-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
  exit 1
}

$rect = New-Object Win32Capture+RECT
[void][Win32Capture]::GetWindowRect($hwnd, [ref]$rect)
$outerW = $rect.Right - $rect.Left
$outerH = $rect.Bottom - $rect.Top
$borderW = [Math]::Max(0, $outerW - [int][Math]::Round($viewport.Width * $scale))
$borderH = [Math]::Max(0, $outerH - [int][Math]::Round($viewport.Height * $scale))
Write-Output "DPI $dpi (scale $scale): outer ${outerW}x${outerH}, viewport $($viewport.Width)x$($viewport.Height) CSS, chrome ${borderW}x${borderH}"

# The filename records the size that was asked for, not the pixel the OS settled on after the
# adjustment -- a report has to be able to say "this is the 1600x900 capture".
$targetW = $Width
$targetH = $Height
$outerW = [int][Math]::Round($Width * $scale) + $borderW
$outerH = [int][Math]::Round($Height * $scale) + $borderH
[void][Win32Capture]::SetWindowPos($hwnd, [IntPtr]::Zero, 40, 40, $outerW, $outerH, 0x0040)
Start-Sleep -Seconds 3

$verify = & node $setRoute $routeNames[0] $Port $SettleMs 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { $failures++ }
Write-Output $verify.Trim()
$viewport = Get-Viewport $verify
if ($viewport -and ($viewport.Width -ne $Width -or $viewport.Height -ne $Height)) {
  Write-Output "Adjusting: viewport is $($viewport.Width)x$($viewport.Height), wanted ${Width}x${Height}"
  $outerW += [int][Math]::Round(($Width - $viewport.Width) * $scale)
  $outerH += [int][Math]::Round(($Height - $viewport.Height) * $scale)
  [void][Win32Capture]::SetWindowPos($hwnd, [IntPtr]::Zero, 40, 40, $outerW, $outerH, 0x0040)
  Start-Sleep -Seconds 3
}

[void][Win32Capture]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 400
[void][Win32Capture]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
Write-Output "Capturing ${w}x${h}"

foreach ($route in $routeNames) {
  if ($route -ne $routeNames[0]) {
    # stdout carries the route line (which viewport the page laid out); stderr carries any browser
    # console errors. Both are printed, and `$LASTEXITCODE` -- not the function's output -- is what
    # counts a route as failed.
    $lines = & node $setRoute $route $Port $SettleMs 2>&1
    foreach ($line in $lines) { Write-Output $line }
    if ($LASTEXITCODE -ne 0) { $failures++ }
  }

  # Route changes are a React render plus whatever the panel fetches; give both the frame they need.
  Start-Sleep -Milliseconds 400

  [void][Win32Capture]::SetForegroundWindow($hwnd)
  Start-Sleep -Milliseconds 250

  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
  $hdc = $g.GetHdc()
  $ok = [Win32Capture]::PrintWindow($hwnd, $hdc, [Win32Capture]::PW_RENDERFULLCONTENT)
  $g.ReleaseHdc($hdc)
  $g.Dispose()

  $path = Join-Path $OutDir ("tauri-{0}-{1}x{2}.png" -f $route, $targetW, $targetH)
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Write-Output "PrintWindow ok: $ok -> $path ($((Get-Item $path).Length) bytes)"
}

Get-Process probelab-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $null

if ($failures -gt 0) {
  Write-Output "FAILED routes: $failures"
  exit 1
}
