/*
Author: https://github.com/bmisiak
License: GNU LESSER GENERAL PUBLIC LICENSE. Version 3, 29 June 2007
*/

/* 
    # SetPreciseTimer returns timer_number, or 0 on failure.
        interval: miliseconds before the callback is triggered.
        repeat:  true/false. If false, the callback will only be called once.

    # Examples:
    SetPreciseTimer("EveryMinute",60000,true);
    SetPreciseTimer("EverySecond",1000,true,"ds",playerid,"Hello");
    new array[2] = {42,2}; SetPreciseTimer("AfterFiveSecs",5000,false,"dsaA",playerid,"world",array,sizeof(array));
*/
native SetPreciseTimer(const callback_name[], const interval, const repeat, const types_of_arguments[]="", {Float,_}:...);

/*
    # DeletePreciseTimer returns 1 if the timer existed or 0 on failure.
*/
native DeletePreciseTimer(const timer_number);