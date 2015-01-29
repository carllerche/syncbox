(function() {var implementors = {};
implementors['syncbox'] = ["<a class='stability Unstable' title='Unstable: will be overhauled with new lifetime rules; see RFC 458'></a>impl&lt;P: Partial&lt;R&gt;, R: <a class='trait' href='http://doc.rust-lang.org/nightly/core/marker/trait.Send.html' title='core::marker::Send'>Send</a>, E: <a class='trait' href='http://doc.rust-lang.org/nightly/core/marker/trait.Send.html' title='core::marker::Send'>Send</a>&gt; <a class='trait' href='http://doc.rust-lang.org/nightly/core/marker/trait.Sync.html' title='core::marker::Sync'>Sync</a> for <a class='struct' href='http://doc.rust-lang.org/nightly/core/cell/struct.UnsafeCell.html' title='core::cell::UnsafeCell'>UnsafeCell</a>&lt;ProgressInner&lt;P, R, E&gt;&gt;","<a class='stability Unstable' title='Unstable: will be overhauled with new lifetime rules; see RFC 458'></a>impl&lt;V, S, E&gt; <a class='trait' href='http://doc.rust-lang.org/nightly/core/marker/trait.Sync.html' title='core::marker::Sync'>Sync</a> for <a class='struct' href='http://doc.rust-lang.org/nightly/core/cell/struct.UnsafeCell.html' title='core::cell::UnsafeCell'>UnsafeCell</a>&lt;Core&lt;V, S, E&gt;&gt;",];

            if (window.register_implementors) {
                window.register_implementors(implementors);
            } else {
                window.pending_implementors = implementors;
            }
        
})()
