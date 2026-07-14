/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/i2c-algo-bit.h> shim (MPL-2.0, original work).
 *
 * GPIO bit-banged I2C algorithm — amdgpu drives DDC/EDID over the connector's
 * I2C lines with it. Display-path; reached via amdgpu_mode.h for type layout. The
 * algorithm (clocking SDA/SCL) is backed by raeen_linuxkpi at M4 when the display
 * path is actually exercised; here it is the struct + registration surface.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_I2C_ALGO_BIT_H
#define _LINUXKPI_LINUX_I2C_ALGO_BIT_H

#include <linux/types.h>

struct i2c_adapter;

struct i2c_algo_bit_data {
	void *data;
	void (*setsda)(void *data, int state);
	void (*setscl)(void *data, int state);
	int  (*getsda)(void *data);
	int  (*getscl)(void *data);
	int  (*pre_xfer)(struct i2c_adapter *);
	void (*post_xfer)(struct i2c_adapter *);
	int  udelay;
	int  timeout;
};

int i2c_bit_add_bus(struct i2c_adapter *adap);
int i2c_bit_add_numbered_bus(struct i2c_adapter *adap);

#endif /* _LINUXKPI_LINUX_I2C_ALGO_BIT_H */
