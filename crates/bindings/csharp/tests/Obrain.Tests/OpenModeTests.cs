using Xunit;

namespace Obrain.Tests;

/// <summary>Decision D4: Open is read-only; OpenWrite takes the write lease (bail).</summary>
public sealed class OpenModeTests : IDisposable
{
    private readonly string _dir = Path.Combine(
        Path.GetTempPath(), "obrain-d4-" + Guid.NewGuid().ToString("N"), "store");

    public void Dispose()
    {
        var root = Path.GetDirectoryName(_dir)!;
        if (Directory.Exists(root))
            Directory.Delete(root, recursive: true);
    }

    [Fact]
    public void OpenWriteCreatesThenOpenReadsBack()
    {
        using (var w = ObrainDB.OpenWrite(_dir))
        {
            w.Execute("INSERT (:Person {name: 'Alix'})");
        }

        using var r = ObrainDB.Open(_dir);
        Assert.Equal(1, r.NodeCount);
    }

    [Fact]
    public void OpenReadOnlyRejectsMutation()
    {
        using (var w = ObrainDB.OpenWrite(_dir))
        {
            w.Execute("INSERT (:Person {name: 'Alix'})");
        }

        using var r = ObrainDB.Open(_dir);
        Assert.ThrowsAny<ObrainException>(() => r.Execute("INSERT (:Person {name: 'Bob'})"));
    }

    [Fact]
    public void OpenMissingStoreThrowsAndCreatesNothing()
    {
        Assert.ThrowsAny<ObrainException>(() => ObrainDB.Open(_dir));
        Assert.False(Directory.Exists(_dir));
    }

    [Fact]
    public void SecondOpenWriteOnSameStoreThrowsWhileLeaseHeld()
    {
        using var first = ObrainDB.OpenWrite(_dir);
        Assert.ThrowsAny<ObrainException>(() => ObrainDB.OpenWrite(_dir));
    }
}
