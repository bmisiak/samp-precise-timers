/*
Author: https://github.com/bmisiak
License: GNU LESSER GENERAL PUBLIC LICENSE. Version 3, 29 June 2007
*/

native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
native DeletePreciseTimer(const timer_number);