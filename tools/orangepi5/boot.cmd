bootdev hunt ethernet
setenv ipaddr 192.168.22.102
setenv serverip 192.168.22.101
setenv netmask 255.255.254.0
setenv gatewayip 192.168.22.1
tftp 0x400000 kernel.uimg
tftp 0x300000 rk3588-orangepi-5-plus.dtb
bootm 0x400000 - 0x300000