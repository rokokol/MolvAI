// SPDX-License-Identifier: MIT
// The media of one result: a video player, or an audio player with a spectrogram of the whole
// file. Both load the file through Tauri's asset protocol, which answers range requests, so a
// seek reads only the bytes it needs. The text of a file the window cannot decode stays
// reachable: the transcript and the open action are outside this component.

import { useEffect, useRef, useState } from "react";

import { asCommandError } from "../api";
import { useI18n } from "../i18n";
import type { MediaInfo } from "../types";
import type { SeekRequest } from "./jobs";
import Spectrogram from "./SpectrogramView";

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
  const [durationMs, setDurationMs] = useState(0);
  const [currentMs, setCurrentMs] = useState(0);

  // Only the file the user expanded becomes readable: the scope of the asset protocol starts
  // empty and this command widens it by one canonical path
  useEffect(() => {
    let cancelled = false;
    setInfo(null);
    setError(null);
    setPlaybackError(null);
    setDurationMs(0);
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

  useEffect(() => {
    const media = mediaRef.current;
    if (!media || !seek) {
      return;
    }
    media.currentTime = seek.ms / 1000;
    // The highlight must move at once, not with the next `timeupdate`
    setCurrentMs(seek.ms);
    onTime(seek.ms);
  }, [seek, info, onTime]);

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

  const handleMetadata = (element: HTMLMediaElement) => {
    setDurationMs(
      Number.isFinite(element.duration) ? Math.round(element.duration * 1000) : 0,
    );
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
        onLoadedMetadata={(event) => handleMetadata(event.currentTarget)}
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
        onLoadedMetadata={(event) => handleMetadata(event.currentTarget)}
        onError={(event) => handleError(event.currentTarget)}
      />
      <Spectrogram
        url={info.url}
        durationMs={durationMs}
        currentMs={currentMs}
        onSeek={(ms) => {
          const media = mediaRef.current;
          if (media) {
            media.currentTime = ms / 1000;
          }
          setCurrentMs(ms);
          onTime(ms);
        }}
      />
    </div>
  );
}
