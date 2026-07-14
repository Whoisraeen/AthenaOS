/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/hdmi.h> shim (MPL-2.0, original work).
 *
 * HDMI infoframe definitions (per the HDMI/CEA-861 spec — the public spec is the
 * source, not GPL Linux). Reached only via the DRM connector type graph that
 * amdgpu_device's embedded drm_device drags in for layout; the MES bring-up path
 * sends no infoframes. Pure data structures + enums (no backing); the pack/unpack
 * helpers are declared for if display code is ever compiled. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_HDMI_H
#define _LINUXKPI_LINUX_HDMI_H

#include <linux/types.h>

enum hdmi_infoframe_type {
	HDMI_INFOFRAME_TYPE_VENDOR = 0x81,
	HDMI_INFOFRAME_TYPE_AVI    = 0x82,
	HDMI_INFOFRAME_TYPE_SPD    = 0x83,
	HDMI_INFOFRAME_TYPE_AUDIO  = 0x84,
	HDMI_INFOFRAME_TYPE_DRM    = 0x87,
};

enum hdmi_colorspace {
	HDMI_COLORSPACE_RGB, HDMI_COLORSPACE_YUV422, HDMI_COLORSPACE_YUV444, HDMI_COLORSPACE_YUV420,
};
enum hdmi_colorimetry { HDMI_COLORIMETRY_NONE, HDMI_COLORIMETRY_ITU_601, HDMI_COLORIMETRY_ITU_709, HDMI_COLORIMETRY_EXTENDED };
enum hdmi_extended_colorimetry {
	HDMI_EXTENDED_COLORIMETRY_XV_YCC_601, HDMI_EXTENDED_COLORIMETRY_XV_YCC_709,
	HDMI_EXTENDED_COLORIMETRY_S_YCC_601,  HDMI_EXTENDED_COLORIMETRY_OPYCC_601,
	HDMI_EXTENDED_COLORIMETRY_OPRGB,      HDMI_EXTENDED_COLORIMETRY_BT2020_CONST_LUM,
	HDMI_EXTENDED_COLORIMETRY_BT2020,     HDMI_EXTENDED_COLORIMETRY_RESERVED,
};
enum hdmi_quantization_range { HDMI_QUANTIZATION_RANGE_DEFAULT, HDMI_QUANTIZATION_RANGE_LIMITED, HDMI_QUANTIZATION_RANGE_FULL };
enum hdmi_eotf {
	HDMI_EOTF_TRADITIONAL_GAMMA_SDR, HDMI_EOTF_TRADITIONAL_GAMMA_HDR,
	HDMI_EOTF_SMPTE_ST2084, HDMI_EOTF_BT_2100_HLG,
};
enum hdmi_metadata_type { HDMI_STATIC_METADATA_TYPE1 = 0 };
enum hdmi_picture_aspect { HDMI_PICTURE_ASPECT_NONE, HDMI_PICTURE_ASPECT_4_3, HDMI_PICTURE_ASPECT_16_9 };

struct hdmi_avi_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
	enum hdmi_colorspace colorspace;
	enum hdmi_colorimetry colorimetry;
	enum hdmi_extended_colorimetry extended_colorimetry;
	enum hdmi_quantization_range quantization_range;
	enum hdmi_picture_aspect picture_aspect;
	unsigned char video_code;
};

struct hdmi_drm_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
	enum hdmi_eotf eotf;
	enum hdmi_metadata_type metadata_type;
	struct { u16 x, y; } display_primaries[3];
	struct { u16 x, y; } white_point;
	u16 max_display_mastering_luminance;
	u16 min_display_mastering_luminance;
	u16 max_cll;
	u16 max_fall;
};

struct hdmi_audio_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
	unsigned char channels;
};

/* HDR static metadata (CEA-861.3) the sink reports + drm_connector embeds. */
struct hdr_static_metadata {
	__u8  eotf;
	__u8  metadata_type;
	__u16 max_cll;
	__u16 max_fall;
	__u16 min_cll;
};
struct hdr_sink_metadata {
	__u32 metadata_type;
	union {
		struct hdr_static_metadata hdmi_type1;
	};
};

struct hdmi_spd_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
	char vendor[8];
	char product[16];
};
struct hdmi_vendor_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
	unsigned int  oui;
	__u8          vic;
};
struct hdmi_any_infoframe {
	enum hdmi_infoframe_type type;
	unsigned char version;
	unsigned char length;
};

/* the tagged union drm_connector embeds by value (drm_connector.h `data`). */
union hdmi_infoframe {
	struct hdmi_any_infoframe    any;
	struct hdmi_avi_infoframe    avi;
	struct hdmi_spd_infoframe    spd;
	struct hdmi_vendor_infoframe vendor;
	struct hdmi_audio_infoframe  audio;
	struct hdmi_drm_infoframe    drm;
};

int hdmi_avi_infoframe_init(struct hdmi_avi_infoframe *frame);
int hdmi_drm_infoframe_init(struct hdmi_drm_infoframe *frame);
ssize_t hdmi_avi_infoframe_pack(struct hdmi_avi_infoframe *frame, void *buffer, size_t size);
ssize_t hdmi_drm_infoframe_pack(struct hdmi_drm_infoframe *frame, void *buffer, size_t size);

#endif /* _LINUXKPI_LINUX_HDMI_H */
