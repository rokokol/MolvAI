// SPDX-License-Identifier: MIT
// The media of one result: a video player, or an audio player with the loudness of the whole
// file. Both read the file from the loopback server of the window, which holds one token per
// file it was allowed to hand out, so a seek reads only the bytes it needs. The text of a file
// the window cannot decode stays reachable: the transcript and the open action are outside
// this component.

import { useEffect, useRef, useState } from "react";

import { asCommandError } from "../api";
import { useI18n } from "../i18n";
import type { MediaInfo } from "../types";
import type { SeekRequest } from "./jobs";
import AudioWaveform from "./AudioWaveform";

export interface MediaPreviewProps {
  path: string;
  seek: SeekRequest | null;
  onTime: (ms: number) => void;
  onOpen: (path: string) => void;
  prepare: (path: string) => Promise<MediaInfo>;
}

export default function MediaPreview({
  path,
  seek,
  onTime,
  onOpen,
  prepare,
}: MediaPreviewProps) {
  const { t } = useI18n();
  const mediaRef = useRef<HTMLVideoElement | HTMLAudioElement | null>(null);
  const [info, setInfo] = useState<MediaInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [currentMs, setCurrentMs] = useState(0);

  // Only the file the user expanded becomes readable: the server hands out a URL for one
  // canonical path at a time, and the URL carries the token that unlocks it
  useEffect(() => {
    let cancelled = false;
    setInfo(null);
    setError(null);
    setPlaybackError(null);
    setCurrentMs(0);
    prepare(path)
      .then((value) => {
        if (!cancelled) {
          setInfo(value);
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          // A refused command arrives as an object, not as an Error
          setError(asCommandError(reason).message);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path, prepare]);

  // The callback of the parent changes on every render of the cell, and a seek effect that
  // depended on it ran again on every frame of the recording: the player jumped back to the
  // moment of the last press all the time. The ref keeps the newest callback out of the deps
  const onTimeRef = useRef(onTime);
  onTimeRef.current = onTime;

  useEffect(() => {
    const media = mediaRef.current;
    if (!media || !seek) {
      return;
    }
    // A player that is already at this moment keeps playing: only a new press moves it
    if (Math.abs(media.currentTime * 1000 - seek.ms) < 100) {
      return;
    }
    media.currentTime = seek.ms / 1000;
    // The highlight must move at once, not with the next `timeupdate`
    setCurrentMs(seek.ms);
    onTimeRef.current(seek.ms);
  }, [seek, info]);

  if (error) {
    return (
      <div className="media-error">
        <p className="small muted">{t("files.mediaError", { reason: error })}</p>
        <button type="button" className="chip" onClick={() => onOpen(path)}>
          {t("files.open")}
        </button>
      </div>
    );
  }
  if (!info) {
    return <p className="small muted">{t("files.mediaLoading")}</p>;
  }
  if (playbackError) {
    return (
      <div className="media-error">
        <p className="small muted">{t("files.mediaUnsupported", { reason: playbackError })}</p>
        <button type="button" className="chip" onClick={() => onOpen(path)}>
          {t("files.open")}
        </button>
      </div>
    );
  }

  const handleTime = (element: HTMLMediaElement) => {
    const ms = Math.round(element.currentTime * 1000);
    setCurrentMs(ms);
    onTime(ms);
  };

  /** Move the player to a moment: a press on the waveform and a press on a segment both ask */
  const seekTo = (ms: number) => {
    const media = mediaRef.current;
    if (media) {
      media.currentTime = ms / 1000;
    }
    setCurrentMs(ms);
    onTime(ms);
  };

  const handleError = (element: HTMLMediaElement) => {
    const error = element.error;
    if (!error) {
      setPlaybackError(t("files.mediaErrorUnknown"));
      return;
    }
    setPlaybackError(error.message || t("files.mediaErrorCode", { code: error.code }));
  };

  if (info.kind === "video") {
    return (
      <video
        ref={(element) => {
          mediaRef.current = element;
        }}
        className="media-player"
        controls
        preload="metadata"
        src={info.url}
        onTimeUpdate={(event) => handleTime(event.currentTarget)}
        onSeeked={(event) => handleTime(event.currentTarget)}
        onError={(event) => handleError(event.currentTarget)}
      />
    );
  }

  return (
    <div className="audio-stack">
      <audio
        ref={(element) => {
          mediaRef.current = element;
        }}
        className="media-player"
        controls
        preload="metadata"
        src={info.url}
        onTimeUpdate={(event) => handleTime(event.currentTarget)}
        onSeeked={(event) => handleTime(event.currentTarget)}
        onError={(event) => handleError(event.currentTarget)}
      />
      <AudioWaveform path={path} currentMs={currentMs} onSeek={seekTo} />
    </div>
  );
}
