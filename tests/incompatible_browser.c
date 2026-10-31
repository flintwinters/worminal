#include "gtk/browser_module.h"
G_MODULE_EXPORT const BrowserModule *
worminal_browser_module(unsigned int abi)
{
	(void)abi;
	return NULL;
}
