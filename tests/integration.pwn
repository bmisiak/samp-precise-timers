#include <a_samp>
#include <samp-precise-timers>

#define TEST_TIMEOUT_MS (5000)

new g_repeat_timer;
new g_delete_timer;
new g_repeat_calls;
new bool:g_arguments_passed;
new bool:g_delete_passed;
new bool:g_reset_passed;

forward CheckArguments(integer, Float:value, const text[], const values[], size);
forward CheckDelete();
forward CheckReset();
forward NeverCalled();
forward TestTimeout();

stock Fail(const reason[])
{
    printf("PRECISE_TIMERS_TEST_FAIL: %s", reason);
    SendRconCommand("exit");
    return 0;
}

stock MaybeFinish()
{
    if (g_arguments_passed && g_delete_passed && g_reset_passed)
    {
        print("PRECISE_TIMERS_TEST_PASS");
        SendRconCommand("exit");
    }
    return 1;
}

public OnGameModeInit()
{
    new values[4] = { 7, -12, 0, 2147483647 };

    if (SetPreciseTimer("MissingPublic", 10, false) != 0)
        return Fail("missing callback was accepted");
    if (SetPreciseTimer("NeverCalled", 0, true) != 0)
        return Fail("zero-interval repeating timer was accepted");
    if (DeletePreciseTimer(2147483647) != 0)
        return Fail("deleting an unknown timer succeeded");
    if (ResetPreciseTimer(2147483647, 10, false) != 0)
        return Fail("resetting an unknown timer succeeded");

    if (!SetPreciseTimer("CheckArguments", 20, false, "ifsai",
        42, 3.25, "timer payload", values, sizeof(values)))
        return Fail("argument timer was not created");

    g_repeat_timer = SetPreciseTimer("CheckReset", 20, true);
    if (!g_repeat_timer)
        return Fail("repeating timer was not created");

    g_delete_timer = SetPreciseTimer("CheckDelete", 20, true);
    if (!g_delete_timer)
        return Fail("deletion timer was not created");

    SetTimer("TestTimeout", TEST_TIMEOUT_MS, false);
    return 1;
}

public CheckArguments(integer, Float:value, const text[], const values[], size)
{
    if (integer != 42 || floatabs(value - 3.25) > 0.0001)
        return Fail("primitive argument changed");
    if (strcmp(text, "timer payload") != 0)
        return Fail("string argument changed");
    if (size != 4 || values[0] != 7 || values[1] != -12 ||
        values[2] != 0 || values[3] != 2147483647)
        return Fail("array argument changed");

    g_arguments_passed = true;
    return MaybeFinish();
}

public CheckDelete()
{
    static calls;
    calls++;
    if (calls != 1)
        return Fail("deleted repeating timer fired again");

    // The timer is rescheduled before its callback, so deleting it from the
    // callback must be safe and must cancel that rescheduled occurrence.
    if (!DeletePreciseTimer(g_delete_timer))
        return Fail("repeating timer could not delete itself");

    g_delete_passed = true;
    return MaybeFinish();
}

public CheckReset()
{
    g_repeat_calls++;
    if (g_repeat_calls == 1)
    {
        if (!ResetPreciseTimer(g_repeat_timer, 30, false))
            return Fail("repeating timer could not reset itself");
        return 1;
    }
    if (g_repeat_calls != 2)
        return Fail("reset one-shot timer fired too many times");

    // One-shot timers are removed before execution, so the expired ID must
    // already be invalid from inside its callback.
    if (DeletePreciseTimer(g_repeat_timer) != 0)
        return Fail("expired one-shot timer remained addressable");

    g_reset_passed = true;
    return MaybeFinish();
}

public NeverCalled()
{
    return Fail("rejected timer callback ran");
}

public TestTimeout()
{
    return Fail("test timed out");
}

main() {}
