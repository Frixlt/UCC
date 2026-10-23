#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Compile the patched driver's actual callbacks against a mock USB/LED device.

Usage: python3 tests/test_ite8291_brightness.py /path/to/patched/tuxedo-drivers
The driver source keeps its original GPL-2.0 license. No copied driver is bundled.
"""
import pathlib
import re
import subprocess
import sys
import tempfile

source = (pathlib.Path(sys.argv[1]) / 'src/ite_8291/ite_8291.c').read_text()
source = re.sub(r'/\*.*?\*/', '', source, flags=re.S)

def function(signature):
    start = source.index(signature + ' {') if signature + ' {' in source else source.index(signature + '\n{')
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]

callback = function('static void leds_set_brightness_mc (struct led_classdev *led_cdev, enum led_brightness brightness)')
flush = function('static int ite8291_perkey_write_state(struct hid_device *hdev)')
shim = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
typedef uint8_t u8;
#define ITE8291_NR_ROWS 6
#define ITE8291_LEDS_PER_ROW_MAX 21
#define ITE8291_ROW_DATA_LENGTH 65
#define pr_debug(...) ((void)0)
enum led_brightness { LED_OFF = 0 };
struct device { struct device *parent; };
struct led_classdev { struct device *dev; unsigned brightness; };
struct mc_subled { unsigned channel, intensity; };
struct led_classdev_mc { struct led_classdev led_cdev; struct mc_subled *subled_info; };
typedef u8 row_data_t[6][65];
struct ite8291_driver_data_perkey_t {
  row_data_t row_data; u8 brightness; bool state_dirty;
  struct led_classdev_mc mcled_cdevs[6][21];
};
struct ite8291_driver_data_t { void *device_data; bool device_buffer_input; };
struct hid_device { struct device dev; struct ite8291_driver_data_t *data; };
#define lcdev_to_mccdev(p) ((struct led_classdev_mc *)(p))
#define to_hid_device(p) ((struct hid_device *)(p))
#define hid_get_drvdata(p) ((p)->data)
static int transfers, usb_result;
static int ite8291_write_rows(struct hid_device *h, row_data_t rows, u8 brightness) {
  (void)h; (void)rows; (void)brightness; ++transfers; return usb_result;
}
// Deliberately scale the channels: comparisons must use encoded row bytes.
static int row_data_set(struct hid_device *h, row_data_t data, int row, int col, int r, int g, int b) {
  (void)h; data[row][44+col]=r/2; data[row][23+col]=g/2; data[row][2+col]=b/2; return 0;
}
static int ite8291_perkey_write_state(struct hid_device *);
'''
main = r'''
int main(void) {
  struct ite8291_driver_data_perkey_t keys = {0};
  struct ite8291_driver_data_t driver = {.device_data=&keys};
  struct hid_device hid = {.data=&driver};
  struct device dev = {.parent=&hid.dev};
  struct mc_subled colors[6][21][3] = {0};
  for (int r=0;r<6;++r) for (int c=0;c<21;++c) {
    struct led_classdev_mc *led=&keys.mcled_cdevs[r][c];
    led->led_cdev.dev=&dev; led->subled_info=colors[r][c];
    colors[r][c][0].channel=r*21+c;
    for (int k=0;k<3;++k) colors[r][c][k].intensity=200-k*20;
    row_data_set(&hid,keys.row_data,r,c,200,180,160);
  }
  row_data_t before; memcpy(before, keys.row_data,sizeof(before));
  // A real UPower-style write to every exposed LED requires one USB frame.
  for (int r=0;r<6;++r) for (int c=0;c<21;++c)
    leds_set_brightness_mc(&keys.mcled_cdevs[r][c].led_cdev,20);
  assert(transfers==1); assert(!memcmp(before,keys.row_data,sizeof(before)));
  // Repeating the same value costs no USB traffic, despite color scaling.
  leds_set_brightness_mc(&keys.mcled_cdevs[2][8].led_cdev,20); assert(transfers==1);
  colors[2][8][0].intensity=40;
  leds_set_brightness_mc(&keys.mcled_cdevs[2][8].led_cdev,20); assert(transfers==2);
  // Failed flushes remain dirty and are retried on an otherwise equal write.
  usb_result=-1;
  leds_set_brightness_mc(&keys.mcled_cdevs[2][8].led_cdev,21); assert(transfers==3);
  usb_result=0;
  leds_set_brightness_mc(&keys.mcled_cdevs[2][8].led_cdev,21); assert(transfers==4);
  assert(!keys.state_dirty);
  // Buffered color/brightness edits wait for the explicit flush.
  driver.device_buffer_input=true; colors[1][3][1].intensity=70;
  leds_set_brightness_mc(&keys.mcled_cdevs[1][3].led_cdev,0);
  assert(transfers==4 && keys.state_dirty);
  ite8291_perkey_write_state(&hid); assert(transfers==5 && !keys.state_dirty);
  driver.device_buffer_input=false;
  leds_set_brightness_mc(&keys.mcled_cdevs[1][3].led_cdev,0); assert(transfers==5);
  leds_set_brightness_mc(&keys.mcled_cdevs[1][3].led_cdev,50); assert(transfers==6);
  puts("PASS: global brightness, scaled colors, USB retry, buffering, zero/max");
}
'''
with tempfile.TemporaryDirectory() as temp:
    path = pathlib.Path(temp)
    (path/'test.c').write_text(shim+flush+'\n'+callback+'\n'+main)
    subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',str(path/'test.c'),'-o',str(path/'test')],check=True)
    subprocess.run([str(path/'test')],check=True)
