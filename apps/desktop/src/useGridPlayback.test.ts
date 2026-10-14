/**
 * The playback controller.
 *
 * The two modes are the whole contract, so each is tested against the failure it exists to prevent: a
 * live viewer that falls behind the engine, and a recorded viewer left pointing into a recording that has
 * been replaced.
 */

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { gridFrames, type GridFrame } from "./gridFrames";
import { frameTurn, GRID_PLAYBACK_SPEEDS, useGridPlayback } from "./useGridPlayback";
import { completedEpisode } from "./test-fixtures/gridRecording";

const frames = gridFrames(completedEpisode());

describe("a live recording", () => {
  it("sits on the newest frame rather than on the last one the viewer reached", () => {
    // A live view that renders frame 3 while the engine has committed frame 7 is plausible and wrong, and
    // it is the exact defect a stored position produces.
    const { result } = renderHook(() => useGridPlayback(frames.slice(0, 3), "live"));
    expect(result.current.position).toBe(2);
    expect(result.current.current?.index).toBe(2);
  });

  it("follows the recording as it grows", () => {
    const { result, rerender } = renderHook(
      ({ list }: { list: GridFrame[] }) => useGridPlayback(list, "live"),
      { initialProps: { list: frames.slice(0, 1) } },
    );
    expect(result.current.position).toBe(0);
    rerender({ list: frames });
    expect(result.current.position).toBe(frames.length - 1);
    expect(result.current.current?.position).toEqual({ x: 1, y: 2 });
  });

  it("does not offer a transport over a recording that is still being written", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "live"));
    expect(result.current.steppable).toBe(false);
  });

  it("refuses to be scrubbed, because there is no end to scrub towards yet", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "live"));
    act(() => {
      result.current.setPosition(1);
      result.current.step(-2);
      result.current.toStart();
    });
    expect(result.current.position).toBe(frames.length - 1);
    expect(result.current.playing).toBe(false);
  });
});

describe("a recorded run", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("opens on the first frame rather than the last", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    expect(result.current.position).toBe(0);
    expect(result.current.current?.kind).toBe("turn");
  });

  it("steps one frame at a time and stops at both ends", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    act(() => result.current.step(1));
    expect(result.current.position).toBe(1);
    act(() => result.current.step(-1));
    act(() => result.current.step(-1));
    expect(result.current.position).toBe(0);
    expect(result.current.atStart).toBe(true);
  });

  it("clamps a scrub to the recording rather than past it", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    act(() => result.current.setPosition(999));
    expect(result.current.position).toBe(frames.length - 1);
    act(() => result.current.setPosition(-5));
    expect(result.current.position).toBe(0);
  });

  it("advances on its own while playing, at the chosen speed", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    act(() => {
      result.current.setSpeed("2×");
      result.current.play();
    });
    const step = GRID_PLAYBACK_SPEEDS.find((speed) => speed.label === "2×")?.delayMs ?? 0;
    act(() => void vi.advanceTimersByTime(step));
    expect(result.current.position).toBe(1);
    act(() => void vi.advanceTimersByTime(step));
    expect(result.current.position).toBe(2);
  });

  it("stops at the end of the recording instead of looping", () => {
    // A silent restart would hide the last frame of the episode, which is the frame most worth seeing.
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    act(() => result.current.toEnd());
    act(() => result.current.play());
    expect(result.current.playing).toBe(false);
    expect(result.current.atEnd).toBe(true);
  });

  it("cancels the timer when the viewer pauses, rather than queueing the next frame anyway", () => {
    const { result } = renderHook(() => useGridPlayback(frames, "recorded"));
    act(() => result.current.play());
    act(() => result.current.pause());
    act(() => void vi.advanceTimersByTime(5_000));
    expect(result.current.position).toBe(0);
    expect(result.current.playing).toBe(false);
  });

  it("returns to the start when the recording underneath it is replaced", () => {
    const { result, rerender } = renderHook(
      ({ list }: { list: GridFrame[] }) => useGridPlayback(list, "recorded"),
      { initialProps: { list: frames } },
    );
    act(() => result.current.step(2));
    expect(result.current.position).toBe(2);
    rerender({ list: gridFrames(completedEpisode().slice(0, 2)) });
    expect(result.current.position).toBe(0);
    expect(result.current.playing).toBe(false);
  });

  it("will not play a recording with nothing to step through", () => {
    const { result } = renderHook(() => useGridPlayback([], "recorded"));
    act(() => result.current.play());
    expect(result.current.playing).toBe(false);
    expect(result.current.current).toBeNull();
    expect(result.current.atEnd).toBe(false);
  });
});

describe("frame labelling", () => {
  it("pads a turn to two digits and says so when there is none", () => {
    expect(frameTurn(frames[0])).toBe("01");
    expect(frameTurn(null)).toBe("—");
  });
});