/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/i2c.h> shim (MPL-2.0, original work).
 *
 * I2C bus model. DRM connectors embed `struct i2c_adapter ddc` BY VALUE for
 * DDC/EDID, so the type must be fully defined for layout. Display/DDC is out of
 * the MES subset; the transfer ops are backed by raeen_linuxkpi at M4 if DDC is
 * brought into scope. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_I2C_H
#define _LINUXKPI_LINUX_I2C_H

#include <linux/types.h>
#include <linux/device.h>

struct i2c_adapter;
struct i2c_msg;
struct i2c_client;

struct i2c_algorithm {
	int (*master_xfer)(struct i2c_adapter *adap, struct i2c_msg *msgs, int num);
	u32 (*functionality)(struct i2c_adapter *adap);
};

struct i2c_adapter {
	struct module     *owner;
	const struct i2c_algorithm *algo;
	void              *algo_data;
	char               name[48];
	struct device      dev;       /* embedded by value */
	int                nr;
	int                retries;
	int                timeout;
};

struct i2c_msg {
	u16  addr;
	u16  flags;
	u16  len;
	u8  *buf;
};

#define I2C_M_RD 0x0001

static inline void *i2c_get_adapdata(const struct i2c_adapter *a) { return dev_get_drvdata(&a->dev); }
static inline void  i2c_set_adapdata(struct i2c_adapter *a, void *d) { dev_set_drvdata(&a->dev, d); }

/* transfer/register — backed by raeen_linuxkpi (M4) */
int i2c_transfer(struct i2c_adapter *adap, struct i2c_msg *msgs, int num);
int i2c_add_adapter(struct i2c_adapter *adap);
void i2c_del_adapter(struct i2c_adapter *adap);

#endif /* _LINUXKPI_LINUX_I2C_H */
