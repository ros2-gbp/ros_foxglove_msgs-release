// Generated by https://github.com/foxglove/schemas
// Options: {}

import { Time } from "./Time";

/** A compressed image */
export type CompressedImage = {
  /** Timestamp of image */
  timestamp: Time;

  /** Frame of reference for the image. The origin of the frame is the optical center of the camera. +x points to the right in the image, +y points down, and +z points into the plane of the image. */
  frame_id: string;

  /** Compressed image data */
  data: Uint8Array;

  /**
   * Image format
   * 
   * Supported values: image media types supported by Chrome, such as `webp`, `jpeg`, `png`
   */
  format: string;
};
