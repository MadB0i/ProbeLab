# Captures the real Tauri window, not a browser render.
#
# The whole redesign was judged against browser screenshots with a stubbed IPC boundary, which is
# necessary for layout work and useless as proof: it cannot show whether the native window chrome,
# the real WebView2 font rasterisation, or the actual backend read paths work. This drives the built
# executable and captures its window with PrintWindow, so what lands in the report is the application
# a user would launch.
#
# Usage: capture-tauri.ps1 [-Routes lab,readiness] [-Width 1600] [-Height 900] [-Scale 1.0]
param(
  [string]$Exe = "C:\Users\rupjy\Desktop\probelab-desktop.exe",
  [string]$Routes = "lab,readiness,agents,benchmarks,security,compare,providers,settings,arena,replay,tasks",
  [int]$Width = 1600,
  [int]$Height = 900,
  [double]$Scale = 1.0,
  [string]$OutDir = "C:\Users\rupjy\AppData\Local\Temp\opencode"
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

if (-not ("Win32Capture" -as [type])) {
  Add-Type @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;

public class Win32Capture {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
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

Get-Process probelab-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

$proc = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 7

$proc.Refresh()
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
  Write-Output "FAILED: no main window handle. The process may have exited: $($proc.HasExited)"
  exit 1
}

# PW_RENDERFULLCONTENT is required: without it WebView2 captures as a blank/black surface.
[void][Win32Capture]::ShowWindow($hwnd, 9)   # SW_RESTORE
[void][Win32Capture]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 800

# DPI correction, applied before any sizing decision.
#
# The window is per-monitor DPI aware, so a size passed to SetWindowPos is in *physical* pixels while
# the layout inside the WebView is in CSS pixels. At 125% scaling a 1600x900 physical window lays out a
# 1280x720 CSS viewport and rasterises it at 1600x900 physical. The failure looks like the right-hand
# column simply being cut off, which is why this is computed rather than eyeballed.
#
# To capture a viewport of N CSS pixels the physical window has to be N * (dpi / 96).
$dpi = [Win32Capture]::GetDpiForWindow($hwnd)
$scaleFactor = $dpi / 96.0
$physicalW = [int][Math]::Round($Width * $scaleFactor)
$physicalH = [int][Math]::Round($Height * $scaleFactor)

Write-Output "Window handle: $hwnd  Title: $($proc.MainWindowTitle)"
Write-Output "DPI: $dpi (scale $scaleFactor)"
Write-Output "Requested viewport: ${Width}x${Height} CSS -> physical ${physicalW}x${physicalH}"

# Sized exactly once.
#
# Setting the window twice -- once to probe and once to the target -- leaves WebView2 holding a layout
# computed for the intermediate size, and the capture comes back with the far column missing. One
# SetWindowPos, then wait for the resize to actually propagate into the web view.
[void][Win32Capture]::SetWindowPos($hwnd, [IntPtr]::Zero, 40, 40, $physicalW, $physicalH, 0x0040)
Start-Sleep -Seconds 4

$rect = New-Object Win32Capture+RECT
[void][Win32Capture]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
Write-Output "Window rect: ${w}x${h} at $($rect.Left),$($rect.Top)"

$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$hdc = $g.GetHdc()
$ok = [Win32Capture]::PrintWindow($hwnd, $hdc, [Win32Capture]::PW_RENDERFULLCONTENT)
$g.ReleaseHdc($hdc)
$g.Dispose()

$name = "tauri-{0}x{1}-{2}pct.png" -f $Width, $Height, [int]($scaleFactor * 100)
$path = Join-Path $OutDir $name
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Output "PrintWindow ok: $ok"
Write-Output "Saved: $path  ($((Get-Item $path).Length) bytes)"

Get-Process probelab-desktop -ErrorAction SilentlyContinue | Stop-Process -Force